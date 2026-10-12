//! What a projection declines, and bytes it cannot read: each way a batch can be one replay would
//! apply otherwise than after everything, and damaged bytes, which never read as a projection.

use crate::id::EventId;
use crate::journal::Journal;
use crate::json::{Object, Value};
use crate::replay::replay;

use super::super::{Extended, Projection, Record, project};
use super::same;

const HERE: &str = "0000aa01";

fn id(n: u128) -> String {
    let value = ((1_790_000_000_000 + n) << 80) | (7 << 76) | (2 << 62) | n;
    EventId::from_u128(value).to_string()
}

fn event(n: u128, replica: &str, deps: &[u128], op: &str, issue: u128, data: Object) -> Value {
    let mut event = Object::new();
    event.insert("v".into(), Value::from(1));
    event.insert("id".into(), Value::from(id(n)));
    event.insert("replica".into(), Value::from(replica));
    let deps = deps.iter().map(|dep| Value::from(id(*dep))).collect();
    event.insert("deps".into(), Value::Array(deps));
    event.insert("op".into(), Value::from(op));
    event.insert("issue".into(), Value::from(id(issue)));
    event.insert("data".into(), Value::from(data));
    Value::from(event)
}

fn fields(title: &str) -> Object {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(title));
    let mut data = Object::new();
    data.insert("fields".into(), Value::from(fields));
    data
}

/// A record of the journal of `folder`.
fn record(value: Value, folder: &str, sequenced: bool) -> Record {
    Record {
        value,
        replica: folder.to_owned(),
        origin: format!("events/{folder}/000001.jsonl"),
        sequenced,
    }
}

/// An `init`, issue 2 and a title set on it, by this machine: the frontier is event 3.
fn base() -> Vec<Record> {
    let mut create = fields("first");
    create.insert("key".into(), Value::from("pr-a1"));
    vec![
        record(event(1, HERE, &[], "init", 1, Object::new()), HERE, true),
        record(event(2, HERE, &[1], "create", 2, create), HERE, true),
        record(event(3, HERE, &[2], "set", 2, fields("second")), HERE, true),
    ]
}

fn journal(records: &[Record]) -> Journal {
    let mut journal = Journal::new();
    for r in records {
        journal.add(r.value.clone(), &r.replica, &r.origin, r.sequenced);
    }
    journal
}

/// Whether the projection of `base()` declines `batch`; when it takes it, it must answer as replay.
fn declines(batch: Vec<Record>) -> bool {
    let projection = project(journal(&base())).unwrap();
    let mut all = base();
    all.extend(batch.iter().cloned());
    match projection.extend(batch).unwrap() {
        Extended::Applied(extended) => {
            assert!(same(extended.state(), &replay(journal(&all)).unwrap()));
            false
        }
        Extended::Replay => true,
    }
}

#[test]
fn a_write_on_the_frontier_is_taken() {
    let batch = vec![record(
        event(4, HERE, &[3], "set", 2, fields("third")),
        HERE,
        true,
    )];
    assert!(!declines(batch));
}

#[test]
fn every_batch_replay_applies_otherwise_is_declined() {
    let set = |n, deps: &[u128]| event(n, HERE, deps, "set", 2, fields("x"));
    let mut takeover = Object::new();
    takeover.insert("takeover".into(), Value::from(id(3)));
    let mut no_id = Object::new();
    no_id.insert("op".into(), Value::from("set"));
    let cases: Vec<(&str, Vec<Record>)> = vec![
        (
            "a record of an event held",
            vec![record(set(3, &[2]), HERE, true)],
        ),
        (
            "another machine's record",
            vec![record(set(4, &[3]), "0000bb02", true)],
        ),
        (
            "a record with no id",
            vec![record(Value::from(no_id), HERE, true)],
        ),
        (
            "a takeover",
            vec![record(
                event(4, HERE, &[3], "claim", 2, takeover),
                HERE,
                false,
            )],
        ),
        (
            "a new machine's journal",
            vec![record(
                event(4, "0000ff09", &[3], "set", 2, fields("x")),
                "0000ff09",
                true,
            )],
        ),
        (
            "a cause behind the frontier",
            vec![record(set(4, &[2]), HERE, true)],
        ),
        (
            "a cause no one holds",
            vec![record(set(4, &[3, 99]), HERE, true)],
        ),
        (
            "an event that saw nothing",
            vec![record(set(4, &[]), HERE, true)],
        ),
        (
            "a malformed payload",
            vec![record(
                event(4, HERE, &[3], "set", 2, Object::new()),
                HERE,
                true,
            )],
        ),
        (
            "one id with two contents",
            vec![
                record(set(4, &[3]), HERE, true),
                record(event(4, HERE, &[3], "set", 2, fields("y")), HERE, true),
            ],
        ),
        (
            "a record that does not build on its predecessor",
            vec![
                record(set(4, &[3]), HERE, true),
                record(set(5, &[3]), HERE, true),
            ],
        ),
        (
            "a record that does not build on its predecessor, which sorts after it",
            vec![
                record(set(5, &[3]), HERE, true),
                record(set(4, &[3]), HERE, true),
            ],
        ),
    ];
    for (case, batch) in cases {
        assert!(declines(batch), "{case}");
    }
}

#[test]
fn a_journal_with_anything_replay_reports_takes_nothing_after_it() {
    let mut records = base();
    records.push(record(
        event(4, HERE, &[3], "set", 2, Object::new()),
        HERE,
        true,
    ));
    let projection = project(journal(&records)).unwrap();
    assert!(!projection.extendable());
    let batch = vec![record(
        event(5, HERE, &[4], "set", 2, fields("x")),
        HERE,
        true,
    )];
    assert!(matches!(
        projection.extend(batch).unwrap(),
        Extended::Replay
    ));
}

#[test]
fn damaged_bytes_never_read_as_a_projection() {
    let encoded = project(journal(&base())).unwrap().encode();
    for (section, bytes) in [("state", &encoded.state), ("index", &encoded.index)] {
        for cut in 0..bytes.len() {
            let mut damaged = encoded.clone();
            let target = if section == "state" {
                &mut damaged.state
            } else {
                &mut damaged.index
            };
            target.truncate(cut);
            assert!(
                Projection::decode(&damaged).is_err(),
                "{section} cut at {cut}"
            );
        }
        for at in 0..bytes.len() {
            for flip in [0x01, 0x80, 0xff] {
                let mut damaged = encoded.clone();
                let target = if section == "state" {
                    &mut damaged.state
                } else {
                    &mut damaged.index
                };
                if let Some(byte) = target.get_mut(at) {
                    *byte ^= flip;
                }
                // read or refused, never a panic: the store checks the bytes before it reads them
                let _ = Projection::decode(&damaged);
            }
        }
    }
}
