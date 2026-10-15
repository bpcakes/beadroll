//! `timing --ready N`: what judging readiness takes (roll-lqjj.19) on a state of N issues in three
//! shapes — a chain, each issue the child of the one before, whose first waits for a prerequisite,
//! so that every issue is held through its ancestors; the same chain with nothing waiting, so that
//! every issue is ready and the answer is sorted; and a lattice, rows of [`WIDTH`] issues each the
//! child of two in the row above, the first row waiting for a prerequisite, so that the issues of a
//! row share every ancestor — judged four ways: for every issue at once (`State::readiness`, the
//! analysis alone, and `State::ready`, with its filter and its sort), issue by issue by the first
//! thing found (`State::is_blocked`, the question `bead show` asks of one issue), and by the
//! complete blocker lists (`State::open_blockers`, what every reader built for every issue before,
//! and what `bead blocked` still lists for the issues held). Replay is not measured. The fastest of
//! [`RUNS`] runs counts. Evidence, never a gate.

use std::time::Instant;

use beadroll_core::journal::Journal;
use beadroll_core::json::{Object, Value};
use beadroll_core::query::ReadyScope;
use beadroll_core::replay::replay;
use beadroll_core::state::State;

use super::world::profile;
use crate::workspace::{Failure, json_string};

/// How many times each question is asked; the fastest run counts.
const RUNS: usize = 3;

/// How many issues a row of the lattice holds.
const WIDTH: usize = 4;

/// The machine whose journal the probe's events are in.
const MACHINE: &str = "a";

/// How the issues are linked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Issue 1 the prerequisite of issue 2, and each later issue the child of the one before.
    ChainHeld,
    /// The same chain, waiting for nothing.
    ChainFree,
    /// Issue 1 the prerequisite of the first row, and each later issue the child of two in the row
    /// above it.
    LatticeHeld,
}

impl Shape {
    const ALL: [Shape; 3] = [Shape::ChainHeld, Shape::ChainFree, Shape::LatticeHeld];

    /// The name its figures are recorded under.
    fn name(self) -> &'static str {
        match self {
            Shape::ChainHeld => "chain_held",
            Shape::ChainFree => "chain_free",
            Shape::LatticeHeld => "lattice_held",
        }
    }

    /// The dependencies issue `n` of `issues` is made with, the issues numbered from one: the key
    /// each names, which has its issue already, and the type.
    fn linked(self, n: usize) -> Vec<(String, &'static str)> {
        match self {
            Shape::ChainHeld if n == 2 => vec![(key(1), "blocks")],
            Shape::ChainHeld | Shape::ChainFree if n > 2 => vec![(key(n - 1), "parent-child")],
            Shape::ChainHeld | Shape::ChainFree => Vec::new(),
            Shape::LatticeHeld if n == 1 => Vec::new(),
            Shape::LatticeHeld => {
                // issue n is at (row, column) counting from issue 2
                let (row, column) = ((n - 2) / WIDTH, (n - 2) % WIDTH);
                if row == 0 {
                    return vec![(key(1), "blocks")];
                }
                let above = |column: usize| key(2 + (row - 1) * WIDTH + column % WIDTH);
                vec![
                    (above(column), "parent-child"),
                    (above(column + 1), "parent-child"),
                ]
            }
        }
    }
}

/// The key of issue `n`.
fn key(n: usize) -> String {
    format!("ex-{n}")
}

/// The id of the probe's event number `n`: a UUIDv7, ids in the order of their numbers.
fn id(n: u64) -> String {
    let millis = 1_791_558_245_123 + n;
    format!(
        "{:08x}-{:04x}-7000-8000-{:012x}",
        millis >> 16,
        millis & 0xffff,
        n
    )
}

/// An event as `Tx.emit` writes it: numbered `n`, doing `op` on itself, built on the event before
/// it, as the events of one machine's journal are.
fn event(n: u64, op: &str, data: Object) -> Value {
    let mut actor = Object::new();
    actor.insert("agent".into(), Value::from("timing"));
    actor.insert("thread".into(), Value::from("a"));
    let mut members = Object::new();
    members.insert("v".into(), Value::from(1));
    members.insert("id".into(), Value::from(id(n)));
    members.insert("replica".into(), Value::from(MACHINE));
    let deps: Vec<Value> = (n > 1)
        .then(|| Value::from(id(n - 1)))
        .into_iter()
        .collect();
    members.insert("deps".into(), Value::Array(deps));
    members.insert("actor".into(), Value::from(actor));
    members.insert("op".into(), Value::from(op));
    members.insert("issue".into(), Value::from(id(n)));
    members.insert("data".into(), Value::from(data));
    Value::from(members)
}

/// The create of issue `n`, imported, with a dependency of each type on each key in `linked`,
/// which become edges at once since every key named has its issue.
fn create(n: usize, linked: &[(String, &str)]) -> Object {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(format!("issue {n}")));
    let mut data = Object::new();
    data.insert("key".into(), Value::from(key(n)));
    data.insert("fields".into(), Value::from(fields));
    data.insert("imported".into(), Value::from(true));
    if !linked.is_empty() {
        let deps = (linked.iter())
            .map(|(named, kind)| {
                let mut dep = Object::new();
                dep.insert("depends_on_id".into(), Value::from(named.as_str()));
                dep.insert("type".into(), Value::from(*kind));
                Value::from(dep)
            })
            .collect();
        data.insert("dangling".into(), Value::Array(deps));
    }
    data
}

/// The state of `issues` creates of `shape`, after an `init`.
fn state(shape: Shape, issues: usize) -> Result<State, Failure> {
    let mut data = Object::new();
    data.insert("prefix".into(), Value::from("ex"));
    let mut journal = Journal::new();
    journal.add(event(1, "init", data), MACHINE, "", true);
    for n in 1..=issues {
        let create = create(n, &shape.linked(n));
        journal.add(event(n as u64 + 1, "create", create), MACHINE, "", true);
    }
    replay(journal).map_err(|_| Failure::Failed("the probe needs a key".into()))
}

/// The fastest of [`RUNS`] runs of `ask`, and its answer.
fn fastest(ask: impl Fn() -> usize) -> (f64, usize) {
    let mut best = f64::INFINITY;
    let mut answer = 0;
    for _ in 0..RUNS {
        let clock = Instant::now();
        answer = ask();
        best = best.min(clock.elapsed().as_secs_f64());
    }
    (best, answer)
}

/// One way of asking: the name its figure is recorded under, and how many issues it counts (held,
/// or ready).
type Question<'s> = (&'static str, Box<dyn Fn() -> usize + 's>);

/// The four ways of asking, each on `state` as a whole.
fn questions(state: &State) -> [Question<'_>; 4] {
    [
        (
            "analysis",
            Box::new(|| {
                let readiness = state.readiness();
                state.issues().filter(|i| readiness.is_blocked(i)).count()
            }),
        ),
        ("ready", Box::new(|| state.ready(ReadyScope::All).len())),
        (
            "each",
            Box::new(|| state.issues().filter(|i| state.is_blocked(i)).count()),
        ),
        (
            "lists",
            Box::new(|| {
                (state.issues())
                    .filter(|i| !state.open_blockers(i).is_empty())
                    .count()
            }),
        ),
    ]
}

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(issues: usize, label: Option<&str>) -> Result<String, Failure> {
    let mut figures = String::new();
    for shape in Shape::ALL {
        let state = state(shape, issues)?;
        let name = shape.name();
        for (question, ask) in questions(&state) {
            let (seconds, answer) = fastest(ask);
            let counted = if question == "ready" { "ready" } else { "held" };
            figures.push_str(&format!(
                "\"{name}_{question}_seconds\": {seconds:.4}, \"{name}_{question}_{counted}\": {answer}, "
            ));
        }
    }
    let label = label.map_or_else(
        || format!("readiness of {issues} issues judged four ways"),
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

    /// What each question answers at `issues` issues of `shape`, in the order of `questions`.
    fn answers(shape: Shape, issues: usize) -> Vec<usize> {
        let state = state(shape, issues).unwrap();
        assert_eq!(state.issues().count(), issues);
        questions(&state).iter().map(|(_, ask)| ask()).collect()
    }

    #[test]
    fn the_held_chain_holds_every_issue_but_the_prerequisite() {
        // held by analysis, ready, held issue by issue, held by the lists
        assert_eq!(answers(Shape::ChainHeld, 10), [9, 1, 9, 9]);
    }

    #[test]
    fn the_free_chain_holds_nothing() {
        assert_eq!(answers(Shape::ChainFree, 10), [0, 10, 0, 0]);
    }

    #[test]
    fn the_lattice_holds_every_row_through_two_parents_each() {
        assert_eq!(answers(Shape::LatticeHeld, 13), [12, 1, 12, 12]);
        let state = state(Shape::LatticeHeld, 13).unwrap();
        let parents = |n: usize| -> Vec<String> {
            let issue = state.resolve(&key(n)).unwrap();
            (state.parents_of(issue).iter())
                .map(|parent| parent.key().to_owned())
                .collect()
        };
        assert!(parents(2).is_empty(), "the first row waits for issue 1");
        assert_eq!(
            state.open_blockers(state.resolve(&key(5)).unwrap()).len(),
            1
        );
        assert_eq!(parents(6), [key(2), key(3)]);
        assert_eq!(parents(9), [key(2), key(5)], "the row wraps around");
        assert_eq!(parents(13), [key(6), key(9)]);
    }

    #[test]
    fn every_shape_names_only_keys_that_have_their_issues() {
        for shape in Shape::ALL {
            let state = state(shape, 12).unwrap();
            assert!(
                state.issues().all(|issue| issue.dangling().is_empty()),
                "{shape:?}"
            );
        }
    }
}
