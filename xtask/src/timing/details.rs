//! `timing --details N`: what showing every issue of a state of N issues in full takes
//! (roll-lqjj.20), above all finding the issues that depend on each, on a sparse graph — issue n
//! waits for issue n/2, and every third issue is also related to the one before — that has no
//! parent-child edge, so that no detail walks ancestors and the reverse lookup is what grows. Asked
//! seven ways: every detail with what they share found once for all of them (`State::details_of`,
//! what `bead show`, `bead update`, `bead claim` and `bead takeover` do with the issues they are
//! given, roll-lqjj.24) and every detail alone (`State::detail`, a pass over every edge for each);
//! the reverse lookup alone, once for all (`State::dependents_of`) and as every detail made it
//! before, every issue's edges collected and sorted for each; and as a control, for one issue, its
//! detail alone, and the lookup now and before. Replay is not measured. The fastest of [`RUNS`]
//! runs counts. Evidence, never a gate.

use beadroll_core::journal::Journal;
use beadroll_core::json::{Object, Value};
use beadroll_core::replay::replay;
use beadroll_core::state::{Issue, State};

use super::ready::{MACHINE, create, event, fastest, key};
use super::world::profile;
use crate::workspace::{Failure, json_string};

/// How many times each question is asked; the fastest run counts.
const RUNS: usize = 3;

/// The dependencies issue `n` is made with, the issues numbered from one: it waits for issue
/// n/2, and every third issue is related to the one before it as well.
fn linked(n: usize) -> Vec<(String, &'static str)> {
    let mut linked = Vec::new();
    if n > 1 {
        linked.push((key(n / 2), "blocks"));
    }
    if n > 1 && n.is_multiple_of(3) {
        linked.push((key(n - 1), "related"));
    }
    linked
}

/// The state of `issues` creates, after an `init`.
fn state(issues: usize) -> Result<State, Failure> {
    let mut data = Object::new();
    data.insert("prefix".into(), Value::from("ex"));
    let mut journal = Journal::new();
    journal.add(event(1, "init", data), MACHINE, "", true);
    for n in 1..=issues {
        let create = create(n, &linked(n));
        journal.add(event(n as u64 + 1, "create", create), MACHINE, "", true);
    }
    replay(journal).map_err(|_| Failure::Failed("the probe needs a key".into()))
}

/// How many dependents a detail lists.
fn listed(detail: &Object) -> usize {
    (detail.get("dependents").and_then(Value::as_array)).map_or(0, <[Value]>::len)
}

/// How many issues depend on `issue`, found as every detail found them before roll-lqjj.20: every
/// issue's edges collected and sorted, and those that name `issue` kept.
fn scanned(state: &State, issue: &Issue) -> usize {
    let uid = issue.uid().to_string();
    let mut found = 0;
    for other in state.issues() {
        let mut edges: Vec<&(String, String)> = other.deps().keys().collect();
        edges.sort();
        found += edges.into_iter().filter(|(to, _)| *to == uid).count();
    }
    found
}

/// One way of asking: the name its figure is recorded under, and how many dependents it finds.
type Question<'s> = (&'static str, Box<dyn Fn() -> usize + 's>);

/// The seven ways of asking, each on `state`; the controls ask of the issue a quarter of the way in,
/// which the two at twice its number wait for.
fn questions(state: &State) -> Result<[Question<'_>; 7], Failure> {
    let one = (state.issues().nth(state.issues().count() / 4))
        .ok_or_else(|| Failure::Failed("the probe needs an issue".into()))?;
    Ok([
        (
            "together",
            Box::new(|| {
                let details = state.details_of(state.issues());
                (state.issues())
                    .map(|issue| listed(&details.of(issue)))
                    .sum()
            }),
        ),
        (
            "each",
            Box::new(|| {
                (state.issues())
                    .map(|issue| listed(&state.detail(issue)))
                    .sum()
            }),
        ),
        (
            "lookup",
            Box::new(|| {
                let dependents = state.dependents_of(state.issues());
                (state.issues())
                    .map(|issue| dependents.of(issue).map_or(0, <[_]>::len))
                    .sum()
            }),
        ),
        (
            "scan",
            Box::new(|| (state.issues()).map(|issue| scanned(state, issue)).sum()),
        ),
        ("single", Box::new(move || listed(&state.detail(one)))),
        (
            "single_lookup",
            Box::new(move || {
                let dependents = state.dependents_of([one]);
                dependents.of(one).map_or(0, <[_]>::len)
            }),
        ),
        ("single_scan", Box::new(move || scanned(state, one))),
    ])
}

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(issues: usize, label: Option<&str>) -> Result<String, Failure> {
    let state = state(issues)?;
    let mut figures = String::new();
    for (question, ask) in questions(&state)? {
        let (seconds, answer) = fastest(ask);
        figures.push_str(&format!(
            "\"{question}_seconds\": {seconds:.6}, \"{question}_dependents\": {answer}, "
        ));
    }
    let label = label.map_or_else(
        || format!("details of {issues} issues and their dependents, seven ways"),
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"issues\": {issues}, \"profile\": {}, \"runs\": {RUNS}, {figures}",
        json_string(&label),
        json_string(profile()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_graph_is_sparse_names_only_issues_and_has_no_parents() {
        let state = state(12).unwrap();
        assert_eq!(state.issues().count(), 12);
        for issue in state.issues() {
            assert!(issue.dangling().is_empty(), "{}", issue.key());
            assert!(state.parents_of(issue).is_empty(), "{}", issue.key());
        }
        let dependents = |n: usize| -> Vec<String> {
            let shown = state.detail(state.resolve(&key(n)).unwrap());
            (shown["dependents"].as_array().unwrap().iter())
                .map(|dep| {
                    let part = |name: &str| dep[name].as_str().unwrap().to_owned();
                    format!("{} {}", part("id"), part("dependency_type"))
                })
                .collect()
        };
        assert_eq!(dependents(1), ["ex-2 blocks", "ex-3 blocks"]);
        assert_eq!(
            dependents(5),
            ["ex-6 related", "ex-10 blocks", "ex-11 blocks"]
        );
        assert!(dependents(12).is_empty());
    }

    #[test]
    fn the_seven_ways_find_the_same_dependents() {
        let state = state(30).unwrap();
        let answers: Vec<usize> = (questions(&state).unwrap().iter())
            .map(|(_, ask)| ask())
            .collect();
        // every edge once in all the details, and the dependents of issue 8: 16, 17 and 9
        let edges: usize = (1..=30).map(|n| linked(n).len()).sum();
        assert_eq!(answers, [edges, edges, edges, edges, 3, 3, 3]);
    }
}
