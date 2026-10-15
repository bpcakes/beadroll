//! What replay reports, case by case (roll-lqjj.28): a projection of a journal holding any of it is
//! extended past it as replay applies what comes after, and what a record still to come can change
//! keeps it from extending, or has the batch declined.

use crate::json::{Object, Value};
use crate::replay::replay;

use super::super::{Extended, Record, project};
use super::hostile::{HERE, base, event, fields, id, journal, record};
use super::same;

/// Whether the projection of `base()` and `flawed`, which replay reports `problem` of, takes `batch`
/// as replay applies it after them.
fn taken_after(flawed: &[Record], problem: &str, batch: Vec<Record>) -> bool {
    let mut held = base();
    held.extend(flawed.iter().cloned());
    let projection = project(journal(&held)).unwrap();
    let integrity = &projection.state().integrity;
    assert!(
        integrity
            .iter()
            .any(|entry| entry.problem.contains(problem)),
        "{integrity:?}"
    );
    let extendable = projection.extendable();
    held.extend(batch.iter().cloned());
    match projection.extend(batch).unwrap() {
        Extended::Applied(extended) => {
            assert!(same(extended.state(), &replay(journal(&held)).unwrap()));
            assert!(extendable && extended.extendable());
            true
        }
        Extended::Replay => false,
    }
}

/// A claim on issue 2 by `replica`, built on `deps`, taking over `claim` when it is one.
fn claim(n: u128, replica: &str, deps: &[u128], claim: Option<u128>) -> Record {
    let mut data = Object::new();
    data.insert("mode".into(), Value::from("online"));
    if let Some(claim) = claim {
        data.insert("takeover".into(), Value::from(id(claim)));
    }
    record(event(n, replica, deps, "claim", 2, data), replica, false)
}

#[test]
fn a_journal_with_what_replay_reports_is_extended_past_it() {
    const THERE: &str = "0000bb02";
    let set = |n, replica, deps: &[u128]| event(n, replica, deps, "set", 2, fields("x"));
    let mut bad_fields = Object::new();
    bad_fields.insert("fields".into(), Value::from(5));
    let mut no_data = set(4, HERE, &[3]).as_object().cloned().unwrap();
    no_data.insert("data".into(), Value::from(5));
    let mut changed = set(4, HERE, &[3]).as_object().cloned().unwrap();
    changed.insert("note".into(), Value::from("another content"));
    let cases: Vec<(&str, Vec<Record>)> = vec![
        (
            "unknown operation",
            vec![record(
                event(4, HERE, &[3], "future_op", 2, Object::new()),
                HERE,
                true,
            )],
        ),
        (
            "fields is not an object",
            vec![record(
                event(4, HERE, &[3], "set", 2, bad_fields),
                HERE,
                true,
            )],
        ),
        (
            "data is not an object",
            vec![record(Value::from(no_data), HERE, true)],
        ),
        (
            "event is not an object",
            vec![
                record(Value::from(7), HERE, true),
                record(set(4, HERE, &[3]), HERE, true),
            ],
        ),
        (
            "one event id with different contents",
            vec![
                record(set(4, HERE, &[3]), HERE, true),
                record(Value::from(changed), HERE, true),
            ],
        ),
        (
            "journal chain broken at position 1 of machine 0000bb02",
            vec![
                record(set(4, THERE, &[3]), THERE, true),
                record(set(5, THERE, &[3]), THERE, true),
            ],
        ),
        (
            "takeover of a claim its writer had not seen",
            vec![claim(4, HERE, &[3], None), claim(5, THERE, &[3], Some(4))],
        ),
        (
            "takeover names a claim that is not a claim on this issue",
            vec![claim(4, THERE, &[3], Some(2))],
        ),
    ];
    // each case named by what replay reports of it
    for (problem, flawed) in cases {
        let deps: Vec<u128> = (4..=5)
            .filter(|n| {
                flawed
                    .iter()
                    .any(|r| r.value.get("id") == Some(&Value::from(id(*n))))
            })
            .collect();
        let write = vec![record(set(6, HERE, &deps), HERE, true)];
        assert!(taken_after(&flawed, problem, write), "{problem}");
    }
}

#[test]
fn a_takeover_of_an_id_no_event_has_keeps_the_projection_from_extending() {
    // refused for naming no claim, until an event has the id: then for naming one it had not seen
    let refused = |records: &[Record]| {
        let state = replay(journal(records)).unwrap();
        let entry = state
            .integrity()
            .iter()
            .find(|entry| entry.event == Some(id(4)));
        entry.map(|entry| entry.problem.clone())
    };
    let mut held = base();
    held.push(claim(4, HERE, &[3], Some(9)));
    assert_eq!(
        refused(&held).as_deref(),
        Some("takeover names a claim that is not a claim on this issue")
    );
    let projection = project(journal(&held)).unwrap();
    assert!(!projection.extendable());
    let batch = vec![claim(9, HERE, &[4], None)];
    held.extend(batch.iter().cloned());
    assert_eq!(
        refused(&held).as_deref(),
        Some("takeover of a claim its writer had not seen")
    );
    assert!(matches!(
        projection.extend(batch).unwrap(),
        Extended::Replay
    ));
    // nor anything else
    let mut held = base();
    held.push(claim(4, HERE, &[3], Some(9)));
    let write = vec![record(
        event(5, HERE, &[4], "set", 2, fields("x")),
        HERE,
        true,
    )];
    assert!(!taken_after(&held[3..], "not a claim", write));
}

#[test]
fn an_id_with_two_contents_placed_by_less_than_its_causes_keeps_the_projection_from_extending() {
    const THERE: &str = "0000bb02";
    let two = |deps: &[u128]| {
        let first = event(4, THERE, deps, "set", 2, fields("x"));
        let mut second = first.as_object().cloned().unwrap();
        second.insert("note".into(), Value::from("another content"));
        vec![
            record(first, THERE, true),
            record(Value::from(second), THERE, true),
        ]
    };
    // its causes loop back onto it: released after every other event, and so after events still
    // to come; and a cause the journal does not hold, which it does not wait for and an event still
    // to come can be, named by its own copy, or with no copy of its own (both found in this
    // machine's journal) by every one
    let mut looped = two(&[5]);
    looped.push(record(
        event(5, HERE, &[4], "set", 2, fields("y")),
        HERE,
        true,
    ));
    let foreign = (two(&[3, 99]).into_iter())
        .map(|copy| record(copy.value, HERE, true))
        .collect();
    for flawed in [looped, two(&[3, 99]), foreign] {
        let mut held = base();
        held.extend(flawed.iter().cloned());
        let projection = project(journal(&held)).unwrap();
        assert!(!projection.extendable());
        let integrity = &projection.state().integrity;
        assert!((integrity.iter()).any(|entry| entry.problem.contains("different contents")));
    }
    // placed by all its causes, it is extended past
    let write = vec![record(
        event(5, HERE, &[4], "set", 2, fields("y")),
        HERE,
        true,
    )];
    assert!(taken_after(&two(&[3]), "different contents", write));
}

#[test]
fn a_record_of_an_id_found_only_in_another_machines_journal_is_declined() {
    const THERE: &str = "0000bb02";
    // the other machine's journal begins, then a record it wrote is found in this one's alone
    let mut flawed = vec![record(
        event(4, THERE, &[3], "set", 2, fields("x")),
        THERE,
        true,
    )];
    let copied = event(5, THERE, &[4], "set", 2, fields("y"));
    flawed.push(record(copied.clone(), HERE, true));
    let mut other = copied.as_object().cloned().unwrap();
    other.insert("note".into(), Value::from("another content"));
    // its own record arrives, built on that machine's journal: as it is, or another content
    for own in [copied, Value::from(other)] {
        let mut held = base();
        held.extend(flawed.iter().cloned());
        let batch = vec![record(own, THERE, true)];
        held.extend(batch.iter().cloned());
        // replay settles the id anew, an event or one with two contents: either way it no longer
        // reports a record found in one journal alone
        let after = replay(journal(&held)).unwrap();
        assert!(
            !(after.integrity().iter()).any(|entry| entry.problem.contains("found only in")),
            "{:?}",
            after.integrity()
        );
        assert!(!taken_after(&flawed, "found only in", batch));
    }
}
