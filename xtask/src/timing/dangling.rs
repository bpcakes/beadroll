//! `timing --dangling N`: what replay takes when imports name keys that no issue has yet
//! (roll-lqjj.18) — N creates in one machine's journal, in five shapes: none naming such a key, the
//! control; each naming a key that never arrives; the first half each naming the key of an issue
//! in the second half, which arrives later; the first issue naming the key of every other, which
//! arrive one create at a time; and, its control, the last issue naming the key of every other,
//! which all have their issues already, so that it gets the same edges at once and nothing waits.
//! Replay used to visit every issue still waiting, and each waiting issue's whole list, on every
//! create, so the middle three grew with the square of N. One issue's edges are a vector searched
//! in order (`state::small`, roll-dih8.11), so its N edges cost what the last control measures
//! however they come. The fastest of [`RUNS`] runs counts. Evidence, never a gate.

use std::time::Instant;

use beadroll_core::journal::Journal;
use beadroll_core::json::{Object, Value};
use beadroll_core::replay::replay;
use beadroll_core::state::State;

use super::world::profile;
use crate::workspace::{Failure, json_string};

/// How many times each journal is replayed; the fastest run counts.
const RUNS: usize = 3;

/// The machine whose journal the probe's events are in.
const MACHINE: &str = "a";

/// What the creates name of keys no issue has when they are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Nothing: every dependency an import brings has its issue, as most do.
    Plain,
    /// Each issue a key that no issue ever has.
    Never,
    /// The first half each the key of an issue of the second half, made later.
    Arriving,
    /// The first issue the key of every other, each made later, one create at a time.
    OneAwaitsMany,
    /// The last issue the key of every other, each made already: the same edges, none waiting.
    OneHasMany,
}

impl Shape {
    const ALL: [Shape; 5] = [
        Shape::Plain,
        Shape::Never,
        Shape::Arriving,
        Shape::OneAwaitsMany,
        Shape::OneHasMany,
    ];

    /// The name its figure is recorded under.
    fn name(self) -> &'static str {
        match self {
            Shape::Plain => "plain",
            Shape::Never => "never",
            Shape::Arriving => "arriving",
            Shape::OneAwaitsMany => "one_awaits_many",
            Shape::OneHasMany => "one_has_many",
        }
    }

    /// The keys issue `n` of `issues` names, the issues numbered from one.
    fn named(self, n: usize, issues: usize) -> Vec<String> {
        let half = issues / 2;
        match self {
            Shape::Plain => Vec::new(),
            Shape::Never => vec![format!("ex-gone-{n}")],
            Shape::Arriving if n <= half => vec![key(half + n)],
            Shape::Arriving => Vec::new(),
            Shape::OneAwaitsMany if n == 1 => (2..=issues).map(key).collect(),
            Shape::OneAwaitsMany => Vec::new(),
            Shape::OneHasMany if n == issues => (1..issues).map(key).collect(),
            Shape::OneHasMany => Vec::new(),
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

/// The create of issue `n`, imported, with a `blocks` dependency on each key in `named`.
fn create(n: usize, named: &[String]) -> Object {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(format!("issue {n}")));
    let mut data = Object::new();
    data.insert("key".into(), Value::from(key(n)));
    data.insert("fields".into(), Value::from(fields));
    data.insert("imported".into(), Value::from(true));
    if !named.is_empty() {
        let deps = (named.iter())
            .map(|named| {
                let mut dep = Object::new();
                dep.insert("depends_on_id".into(), Value::from(named.as_str()));
                dep.insert("type".into(), Value::from("blocks"));
                Value::from(dep)
            })
            .collect();
        data.insert("dangling".into(), Value::Array(deps));
    }
    data
}

/// The journal of `issues` creates of `shape`, after an `init`.
fn journal(shape: Shape, issues: usize) -> Journal {
    let mut data = Object::new();
    data.insert("prefix".into(), Value::from("ex"));
    let mut journal = Journal::new();
    journal.add(event(1, "init", data), MACHINE, "", true);
    for n in 1..=issues {
        let value = event(n as u64 + 1, "create", create(n, &shape.named(n, issues)));
        journal.add(value, MACHINE, "", true);
    }
    journal
}

/// How many edges the state's issues have, and how many dependencies still wait for a key.
fn counted(state: &State) -> (usize, usize) {
    (state.issues()).fold((0, 0), |(edges, waiting), issue| {
        (edges + issue.deps().len(), waiting + issue.dangling().len())
    })
}

/// The fastest of [`RUNS`] replays of `journal`, copied before the clock runs, and the edges and
/// waiting dependencies of the state the last one made.
fn fastest(journal: &Journal) -> Result<(f64, (usize, usize)), Failure> {
    let mut best = f64::INFINITY;
    let mut counts = (0, 0);
    for _ in 0..RUNS {
        let trial = journal.clone();
        let clock = Instant::now();
        let state = replay(trial).map_err(|_| Failure::Failed("the probe needs a key".into()))?;
        best = best.min(clock.elapsed().as_secs_f64());
        counts = counted(&state);
    }
    Ok((best, counts))
}

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(issues: usize, label: Option<&str>) -> Result<String, Failure> {
    let mut figures = String::new();
    for shape in Shape::ALL {
        let (seconds, (edges, waiting)) = fastest(&journal(shape, issues))?;
        let name = shape.name();
        figures.push_str(&format!(
            "\"{name}_seconds\": {seconds:.4}, \"{name}_edges\": {edges}, \
             \"{name}_waiting\": {waiting}, "
        ));
    }
    let label = label.map_or_else(
        || format!("replay of {issues} creates naming keys not yet issues"),
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

    /// The keys each issue's edges go to, by issue key, and how many dependencies it still waits on.
    fn shape_of(shape: Shape, issues: usize) -> Vec<(String, Vec<String>, usize)> {
        let state = replay(journal(shape, issues)).unwrap();
        (state.issues())
            .map(|issue| {
                let to: Vec<String> = (issue.deps().keys())
                    .map(|(to, _)| state.key_of(to).to_owned())
                    .collect();
                (issue.key().to_owned(), to, issue.dangling().len())
            })
            .collect()
    }

    #[test]
    fn the_control_names_no_key_that_waits() {
        let state = replay(journal(Shape::Plain, 6)).unwrap();
        assert_eq!(state.issues().count(), 6);
        assert_eq!(counted(&state), (0, 0));
    }

    #[test]
    fn a_key_that_never_arrives_leaves_every_issue_waiting() {
        let state = replay(journal(Shape::Never, 6)).unwrap();
        assert_eq!(
            counted(&state),
            (0, 6),
            "one dependency each, still waiting"
        );
    }

    #[test]
    fn the_first_half_waits_for_the_second_and_each_gets_its_edge() {
        let shape = shape_of(Shape::Arriving, 6);
        let edges: Vec<(&str, Vec<&str>)> = (shape.iter())
            .map(|(key, to, waiting)| {
                assert_eq!(*waiting, 0, "{key} waits for nothing once all are made");
                (key.as_str(), to.iter().map(String::as_str).collect())
            })
            .collect();
        assert_eq!(
            edges,
            [
                ("ex-1", vec!["ex-4"]),
                ("ex-2", vec!["ex-5"]),
                ("ex-3", vec!["ex-6"]),
                ("ex-4", vec![]),
                ("ex-5", vec![]),
                ("ex-6", vec![]),
            ]
        );
    }

    #[test]
    fn one_issue_waits_for_every_other_and_ends_with_an_edge_to_each() {
        let shape = shape_of(Shape::OneAwaitsMany, 6);
        let (key, to, waiting) = &shape[0];
        assert_eq!(key, "ex-1");
        assert_eq!(to, &["ex-2", "ex-3", "ex-4", "ex-5", "ex-6"]);
        assert_eq!(*waiting, 0);
        assert!(
            shape[1..]
                .iter()
                .all(|(_, to, waiting)| to.is_empty() && *waiting == 0)
        );
        // Before the others are made, it waits on all five.
        let mut data = Object::new();
        data.insert("prefix".into(), Value::from("ex"));
        let mut alone = Journal::new();
        alone.add(event(1, "init", data), MACHINE, "", true);
        let waiting = create(1, &Shape::OneAwaitsMany.named(1, 6));
        alone.add(event(2, "create", waiting), MACHINE, "", true);
        assert_eq!(counted(&replay(alone).unwrap()), (0, 5));
    }

    #[test]
    fn its_control_gets_the_same_edges_from_keys_that_have_their_issues() {
        let shape = shape_of(Shape::OneHasMany, 6);
        let (key, to, waiting) = &shape[5];
        assert_eq!(key, "ex-6");
        assert_eq!(to, &["ex-1", "ex-2", "ex-3", "ex-4", "ex-5"]);
        assert_eq!(*waiting, 0);
        assert!(
            shape[..5]
                .iter()
                .all(|(_, to, waiting)| to.is_empty() && *waiting == 0)
        );
    }
}
