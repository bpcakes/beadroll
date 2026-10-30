//! `timing --extend N`: what applying events to a projection takes on the shape the finishing
//! pass was worst for (roll-lqjj.17) — a tracker of N issues, each with two labels and a parent in
//! the tenth before it, and then N events applied to it one at a time, as a command that writes
//! one event per issue and reads the state between them does, against the same events in one
//! batch. It measures closes, which change neither labels nor edges; label additions, which change
//! the labels of the issue each names; and dependency additions, which the parent-child index is
//! made of, so one of them has it built again over the whole state. Two shapes measure what the
//! finishing pass and the ids cost when a batch is not one event per issue in clock order: N label
//! additions to one issue in a single batch, whose labels are derived once rather than once per
//! event, and N closes one at a time whose ids all sort before every id held, as a machine's own
//! writes do after its clock rolls back or a peer's clock runs ahead (Z02 follow-up). And N closes
//! one at a time to the tracker with an event of an operation a newer bead writes after its last
//! create, which replay reports and sets aside: a projection holding it is extended all the same
//! (roll-lqjj.28). The fastest of [`RUNS`] runs counts. Evidence, never a gate.

use std::time::Instant;

use beadroll_core::journal::Journal;
use beadroll_core::journal::JournalOrder;
use beadroll_core::json::{Object, Value};
use beadroll_core::projection::{Extended, Projection, Record, project};

use super::world::profile;
use crate::workspace::{Failure, json_string};

/// How many times each measurement is made; the fastest run counts.
const RUNS: usize = 3;

/// The machine whose journal the probe's events are in.
const MACHINE: &str = "a";

/// The id of the probe's event number `n`: a UUIDv7, ids in the order of their numbers, so that
/// what a machine writes comes after every event it holds.
pub(super) fn id(n: u64) -> String {
    stamped(1_791_558_245_123, n)
}

/// The id of event `n` written a year earlier: before every id [`id`] gives, whatever `n` is, as a
/// machine's own write is after its clock rolls back or when it takes in a peer that ran ahead.
fn earlier(n: u64) -> String {
    stamped(1_759_935_845_123, n)
}

/// A UUIDv7 at `millis`, numbered `n`.
fn stamped(millis: u64, n: u64) -> String {
    format!(
        "{:08x}-{:04x}-7000-8000-{:012x}",
        millis >> 16,
        millis & 0xffff,
        n
    )
}

/// An event as `Tx.emit` writes it: numbered `n`, doing `op` on `issue`, built on `after` alone,
/// as the events of one machine's journal are.
fn event(n: u64, op: &str, issue: &str, after: Option<u64>, data: Object) -> Value {
    let mut actor = Object::new();
    actor.insert("agent".into(), Value::from("timing"));
    actor.insert("thread".into(), Value::from("a"));
    let mut members = Object::new();
    members.insert("v".into(), Value::from(1));
    members.insert("id".into(), Value::from(id(n)));
    members.insert("replica".into(), Value::from(MACHINE));
    let deps: Vec<Value> = (after.iter())
        .map(|earlier| Value::from(id(*earlier)))
        .collect();
    members.insert("deps".into(), Value::Array(deps));
    members.insert("actor".into(), Value::from(actor));
    members.insert("op".into(), Value::from(op));
    members.insert("issue".into(), Value::from(issue));
    members.insert("data".into(), Value::from(data));
    Value::from(members)
}

/// An event as [`event`] writes it, but identified by [`earlier`]: its id sorts before every id of
/// the tracker, and it still builds on `after`, which is one of them.
fn event_earlier(n: u64, op: &str, issue: &str, after: Option<&str>, data: Object) -> Value {
    let mut members = match event(n, op, issue, None, data) {
        Value::Object(members) => *members,
        other => return other,
    };
    members.insert("id".into(), Value::from(earlier(n)));
    let deps = (after.into_iter()).map(Value::from).collect();
    members.insert("deps".into(), Value::Array(deps));
    Value::from(members)
}

/// A record of that machine's journal.
fn record(value: Value) -> Record {
    Record {
        value,
        replica: MACHINE.to_owned(),
        origin: String::new(),
        sequenced: true,
    }
}

/// The number of the event that creates issue `n`, the issues numbered from one: the `init` is
/// event one, so each create follows the one before it.
pub(super) fn created(n: usize) -> u64 {
    n as u64 + 1
}

/// The create of issue `n`: two labels, and, unless it is the first of its ten, an imported
/// dependency on the key of that first, which becomes a parent-child edge at once because the key
/// already has its issue. The shape leaves a tenth of the issues a parent of nine.
fn create(n: usize) -> Object {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(format!("issue {n}")));
    fields.insert("status".into(), Value::from("open"));
    let mut data = Object::new();
    data.insert("key".into(), Value::from(format!("ex-{n}")));
    data.insert("fields".into(), Value::from(fields));
    data.insert(
        "labels".into(),
        Value::Array(vec![Value::from("ui"), Value::from("db")]),
    );
    let first = n - (n - 1) % 10;
    if n != first {
        let mut dep = Object::new();
        dep.insert("depends_on_id".into(), Value::from(format!("ex-{first}")));
        dep.insert("type".into(), Value::from("parent-child"));
        data.insert("dangling".into(), Value::Array(vec![Value::from(dep)]));
        data.insert("imported".into(), Value::from(true));
    }
    data
}

/// The journal of the tracker the probe applies events to: `issues` issues, in one machine's
/// journal, each event built on the one before it.
fn journal(issues: usize) -> Journal {
    let mut data = Object::new();
    data.insert("prefix".into(), Value::from("ex"));
    let mut journal = Journal::new();
    journal.add(
        event(1, "init", &id(1), None, data),
        MACHINE,
        "",
        JournalOrder::Sequenced,
    );
    for n in 1..=issues {
        let number = created(n);
        let value = event(number, "create", &id(number), Some(number - 1), create(n));
        journal.add(value, MACHINE, "", JournalOrder::Sequenced);
    }
    journal
}

/// That tracker, replayed.
pub(super) fn tracker(issues: usize) -> Result<Projection, Failure> {
    replayed(journal(issues))
}

/// That tracker with one event more, built on its last create: of an operation a newer bead writes
/// and this one does not know, which replay reports and sets aside.
fn tracker_past_newer(issues: usize) -> Result<Projection, Failure> {
    let mut journal = journal(issues);
    let number = created(issues) + 1;
    let value = event(
        number,
        "archive",
        &id(number),
        Some(number - 1),
        Object::new(),
    );
    journal.add(value, MACHINE, "", JournalOrder::Sequenced);
    replayed(journal)
}

/// `journal` replayed into a projection.
fn replayed(journal: Journal) -> Result<Projection, Failure> {
    project(journal).map_err(|_| Failure::Failed("the probe's own tracker does not replay".into()))
}

/// What the probe applies to that tracker: `issues` events doing `op`, one per issue, each built on
/// the one before it, the first on the tracker's last event. `payload` makes each one's data from
/// the number of the issue it is about.
fn events(issues: usize, op: &str, payload: impl Fn(usize) -> Object) -> Vec<Record> {
    events_after(created(issues), issues, op, payload)
}

/// [`events`] after the tracker's event numbered `last`.
fn events_after(
    last: u64,
    issues: usize,
    op: &str,
    payload: impl Fn(usize) -> Object,
) -> Vec<Record> {
    (1..=issues)
        .map(|n| {
            let number = last + n as u64;
            let issue = id(created(n));
            record(event(number, op, &issue, Some(number - 1), payload(n)))
        })
        .collect()
}

/// The `issues` closes again, each identified before every id the tracker holds and chained after
/// the one before it, the first on the tracker's last event: ids that are never appended.
pub(super) fn closes_before_held(issues: usize, head: &str) -> Vec<Record> {
    (1..=issues)
        .map(|n| {
            let number = created(issues) + n as u64;
            let issue = id(created(n));
            let after = if n == 1 {
                head.to_owned()
            } else {
                earlier(number - 1)
            };
            record(event_earlier(
                number,
                "close",
                &issue,
                Some(&after),
                closing(n),
            ))
        })
        .collect()
}

/// `issues` label additions to the first issue alone, chained: one batch that changes one issue's
/// labels over and over, where the labels derived are those it holds once they have all applied.
fn churn(issues: usize) -> Vec<Record> {
    let issue = id(created(1));
    (1..=issues)
        .map(|n| {
            let number = created(issues) + n as u64;
            record(event(number, "label_add", &issue, Some(number - 1), {
                let mut data = Object::new();
                data.insert(
                    "labels".into(),
                    Value::Array(vec![Value::from(format!("c{n}"))]),
                );
                data
            }))
        })
        .collect()
}

/// A close, with the reason a `bead close -r` records.
fn closing(_: usize) -> Object {
    let mut data = Object::new();
    data.insert("reason".into(), Value::from("done"));
    data
}

/// A label added that the issue does not have, so that its labels change.
fn labelling(n: usize) -> Object {
    let mut data = Object::new();
    data.insert(
        "labels".into(),
        Value::Array(vec![Value::from(format!("l{n}"))]),
    );
    data
}

/// A `blocks` dependency on the issue after this one, the last on the first: an edge the
/// parent-child index is built again for, though it holds no parent-child edge of its own.
fn wiring(issues: usize) -> impl Fn(usize) -> Object {
    move |n| {
        let mut data = Object::new();
        let next = n % issues.max(1) + 1;
        data.insert("to".into(), Value::from(id(created(next))));
        data.insert("type".into(), Value::from("blocks"));
        data.insert("meta".into(), Value::from(Object::new()));
        data
    }
}

/// `records` applied to `seed`, one event at a time: the projection they make.
pub(super) fn one_at_a_time(seed: Projection, records: Vec<Record>) -> Result<Projection, Failure> {
    let mut projection = seed;
    for record in records {
        projection = applied(projection, vec![record])?;
    }
    Ok(projection)
}

/// `records` applied to `projection`, which must take them.
fn applied(projection: Projection, records: Vec<Record>) -> Result<Projection, Failure> {
    match projection.extend(records) {
        Ok(Extended::Applied(extended)) => Ok(*extended),
        Ok(Extended::Replay) => Err(Failure::Failed(
            "the probe's own events are declined, so nothing was measured".into(),
        )),
        Err(_) => Err(Failure::Failed("the probe's own events need a key".into())),
    }
}

/// The fastest of [`RUNS`] runs of `apply` on a fresh copy of `seed` and of `records`, neither
/// copied while the clock runs.
fn fastest(
    seed: &Projection,
    records: &[Record],
    apply: impl Fn(Projection, Vec<Record>) -> Result<Projection, Failure>,
) -> Result<f64, Failure> {
    let mut best = f64::INFINITY;
    for _ in 0..RUNS {
        let (seed, records) = (seed.clone(), records.to_vec());
        let clock = Instant::now();
        let made = apply(seed, records)?;
        best = best.min(clock.elapsed().as_secs_f64());
        drop(made);
    }
    Ok(best)
}

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(issues: usize, label: Option<&str>) -> Result<String, Failure> {
    let seed = tracker(issues)?;
    let closes = events(issues, "close", closing);
    let one_by_one = fastest(&seed, &closes, one_at_a_time)?;
    let in_one_batch = fastest(&seed, &closes, applied)?;
    let closed = one_at_a_time(seed.clone(), closes)?;
    let closed = (closed.state().issues())
        .filter(|issue| issue.status() == "closed")
        .count();
    let labels = fastest(
        &seed,
        &events(issues, "label_add", labelling),
        one_at_a_time,
    )?;
    let churned = fastest(&seed, &churn(issues), applied)?;
    let head = id(created(issues));
    let before_held = fastest(&seed, &closes_before_held(issues, &head), one_at_a_time)?;
    let edges = fastest(
        &seed,
        &events(issues, "dep_add", wiring(issues)),
        one_at_a_time,
    )?;
    let past_newer = fastest(
        &tracker_past_newer(issues)?,
        &events_after(created(issues) + 1, issues, "close", closing),
        one_at_a_time,
    )?;
    let label = label.map_or_else(
        || format!("projection extended on {issues} issues"),
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"issues\": {issues}, \"profile\": {}, \"runs\": {RUNS}, \
         \"closed\": {closed}, \"close_one_by_one_seconds\": {one_by_one:.4}, \
         \"close_in_one_batch_seconds\": {in_one_batch:.4}, \
         \"close_before_held_one_by_one_seconds\": {before_held:.4}, \
         \"label_one_by_one_seconds\": {labels:.4}, \
         \"label_one_issue_in_one_batch_seconds\": {churned:.4}, \
         \"dep_one_by_one_seconds\": {edges:.4}, \
         \"close_past_newer_one_by_one_seconds\": {past_newer:.4}, ",
        json_string(&label),
        json_string(profile()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tracker_holds_every_issue_with_its_labels_and_a_tenth_of_them_a_parent() {
        let seed = tracker(20).unwrap();
        let state = seed.state();
        assert_eq!(state.issues().count(), 20);
        assert!(
            (state.issues()).all(|issue| issue.labels().len() == 2),
            "every issue keeps the labels its create carried"
        );
        let parents: Vec<usize> = (state.issues())
            .filter(|issue| !state.children(issue.uid()).is_empty())
            .map(|issue| state.children(issue.uid()).len())
            .collect();
        assert_eq!(parents, [9, 9], "a tenth of them is a parent of nine");
        assert_eq!(seed.heads().len(), 1, "one machine's journal, one head");
    }

    #[test]
    fn closing_one_at_a_time_and_in_one_batch_make_the_same_state() {
        let seed = tracker(12).unwrap();
        let closes = events(12, "close", closing);
        let singly = one_at_a_time(seed.clone(), closes.clone()).unwrap();
        let batched = applied(seed, closes).unwrap();
        let closed = |projection: &Projection| {
            (projection.state().issues())
                .filter(|issue| issue.status() == "closed")
                .count()
        };
        assert_eq!(closed(&singly), 12);
        assert_eq!(closed(&batched), 12);
        assert_eq!(singly.heads(), batched.heads());
    }

    #[test]
    fn a_churning_batch_changes_one_issue_and_its_ids_come_after_the_trackers() {
        let seed = tracker(6).unwrap();
        let batch = churn(6);
        assert_eq!(batch.len(), 6);
        let issue = id(created(1));
        assert!(
            (batch.iter()).all(|r| r.value.get("issue").and_then(Value::as_str) == Some(&*issue)),
            "every one is about the first issue"
        );
        let churned = applied(seed, batch).unwrap();
        let labels: Vec<&str> = (churned.state().issues())
            .find(|held| held.uid().to_string() == issue)
            .unwrap()
            .labels()
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(labels, ["c1", "c2", "c3", "c4", "c5", "c6", "db", "ui"]);
    }

    #[test]
    fn the_closes_written_earlier_sort_before_every_id_the_tracker_holds() {
        let seed = tracker(6).unwrap();
        let head = id(created(6));
        let batch = closes_before_held(6, &head);
        // `head` is the tracker's last event and its largest id, as [`id`] ascends with `n`.
        for held in &batch {
            let written = held.value.get("id").and_then(Value::as_str).unwrap();
            assert!(written < head.as_str(), "{written} comes before {head}");
        }
        let closed = one_at_a_time(seed, batch).unwrap();
        assert_eq!(
            (closed.state().issues())
                .filter(|issue| issue.status() == "closed")
                .count(),
            6
        );
    }

    #[test]
    fn closes_after_an_event_replay_sets_aside_are_taken_one_at_a_time() {
        let seed = tracker_past_newer(8).unwrap();
        let reported: Vec<&str> = (seed.state().integrity().iter())
            .map(|entry| entry.problem.as_str())
            .collect();
        assert_eq!(reported, ["unknown operation 'archive'"]);
        let closes = events_after(created(8) + 1, 8, "close", closing);
        let closed = one_at_a_time(seed, closes).unwrap();
        assert_eq!(
            (closed.state().issues())
                .filter(|issue| issue.status() == "closed")
                .count(),
            8
        );
    }

    #[test]
    fn a_label_added_is_one_the_issue_lacked_and_an_edge_added_is_one_it_lacked() {
        let seed = tracker(10).unwrap();
        let labelled = one_at_a_time(seed.clone(), events(10, "label_add", labelling)).unwrap();
        assert!(
            (labelled.state().issues()).all(|issue| issue.labels().len() == 3),
            "each issue gains a label of its own"
        );
        let wired = one_at_a_time(seed, events(10, "dep_add", wiring(10))).unwrap();
        let blocks = (wired.state().issues())
            .filter(|issue| (issue.deps().keys()).any(|edge| edge.kind() == "blocks"))
            .count();
        assert_eq!(blocks, 10, "each issue gains an edge of its own");
    }
}
