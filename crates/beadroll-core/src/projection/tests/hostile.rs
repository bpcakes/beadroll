//! What a projection declines, and bytes it cannot read: each way a batch can be one replay would
//! apply otherwise than after everything, and damaged bytes, which never read as a projection.

use crate::id::EventId;
use crate::journal::Journal;
use crate::json::{Object, Value};

use super::super::{Extended, Projection, Record, project};
use super::same;
use super::shown::replayed;

pub(super) const HERE: &str = "0000aa01";

pub(super) fn id(n: u128) -> String {
    let value = ((1_790_000_000_000 + n) << 80) | (7 << 76) | (2 << 62) | n;
    EventId::from_u128(value).to_string()
}

pub(super) fn event(
    n: u128,
    replica: &str,
    deps: &[u128],
    op: &str,
    issue: u128,
    data: Object,
) -> Value {
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

pub(super) fn fields(title: &str) -> Object {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(title));
    let mut data = Object::new();
    data.insert("fields".into(), Value::from(fields));
    data
}

/// A record of the journal of `folder`.
pub(super) fn record(value: Value, folder: &str, sequenced: bool) -> Record {
    Record {
        value,
        replica: folder.to_owned(),
        origin: format!("events/{folder}/000001.jsonl"),
        sequenced,
    }
}

/// An `init`, issue 2 and a title set on it, by this machine: the frontier is event 3.
pub(super) fn base() -> Vec<Record> {
    let mut create = fields("first");
    create.insert("key".into(), Value::from("pr-a1"));
    vec![
        record(event(1, HERE, &[], "init", 1, Object::new()), HERE, true),
        record(event(2, HERE, &[1], "create", 2, create), HERE, true),
        record(event(3, HERE, &[2], "set", 2, fields("second")), HERE, true),
    ]
}

pub(super) fn journal(records: &[Record]) -> Journal {
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
            assert!(same(extended.state(), &replayed(&all)));
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
fn damaged_bytes_never_read_as_a_projection() {
    let (encoded, _) = project(journal(&base())).unwrap().encode(0);
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

/// `base()` with a description on issue 2 and a second issue, 5, created after it: a projection of
/// it read back from its bytes, every issue's texts there, and the bytes its texts are in.
fn with_texts() -> (Projection, Vec<u8>) {
    let mut described = Object::new();
    described.insert(
        "description".into(),
        Value::from("why it matters, at length"),
    );
    let mut data = Object::new();
    data.insert("fields".into(), Value::from(described));
    let mut create = fields("other");
    create.insert("key".into(), Value::from("pr-b2"));
    let mut records = base();
    records.push(record(event(4, HERE, &[3], "set", 2, data), HERE, true));
    records.push(record(
        event(5, HERE, &[4], "create", 5, create),
        HERE,
        true,
    ));
    let (encoded, _) = project(journal(&records)).unwrap().encode(0);
    (Projection::decode(&encoded).unwrap(), encoded.texts)
}

/// The bytes of `file` at `place`.
fn at(file: &[u8], place: crate::state::TextPlace) -> Vec<u8> {
    let start = usize::try_from(place.at).unwrap();
    file[start..start + place.len as usize].to_vec()
}

#[test]
fn a_change_to_texts_not_read_in_is_declined_until_they_are() {
    let comment = || {
        let mut data = Object::new();
        data.insert("text".into(), Value::from("noted"));
        vec![record(event(6, HERE, &[5], "comment", 2, data), HERE, true)]
    };
    let (projection, file) = with_texts();
    assert!(matches!(
        projection.clone().extend(comment()).unwrap(),
        Extended::Replay
    ));
    // a title set needs no texts: taken with them there
    let retitle = vec![record(
        event(6, HERE, &[5], "set", 2, fields("t")),
        HERE,
        true,
    )];
    assert!(matches!(
        projection.clone().extend(retitle).unwrap(),
        Extended::Applied(_)
    ));
    let mut read = projection;
    let uid = EventId::parse(&id(2)).unwrap();
    let wanted = read.state().wanted_texts([uid]);
    for (uid, place) in wanted {
        read.state_mut()
            .read_texts(uid, place, &at(&file, place))
            .unwrap();
    }
    let Extended::Applied(extended) = read.extend(comment()).unwrap() else {
        panic!("the texts were read in");
    };
    let issue = extended.state().issue(uid).unwrap();
    assert_eq!(issue.comments().len(), 1);
    assert_eq!(
        issue.get(crate::event::Field::Description),
        Some(&Value::from("why it matters, at length"))
    );
}

#[test]
fn damaged_texts_are_never_read_in() {
    let (projection, file) = with_texts();
    for (uid, place) in projection.state().all_wanted_texts() {
        let bytes = at(&file, place);
        for cut in 0..bytes.len() {
            let mut state = projection.state().clone();
            assert!(state.read_texts(uid, place, &bytes[..cut]).is_err());
            assert!(!state.issue(uid).unwrap().texts().here(), "nothing read in");
        }
        for at in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[at] ^= 0x01;
            let mut state = projection.state().clone();
            assert!(state.read_texts(uid, place, &damaged).is_err(), "byte {at}");
        }
        let mut state = projection.state().clone();
        state.read_texts(uid, place, &bytes).unwrap();
        assert!(state.issue(uid).unwrap().texts().here());
    }
}

#[test]
fn one_issues_texts_are_never_read_in_as_anothers() {
    let (projection, file) = with_texts();
    let wanted = projection.state().all_wanted_texts();
    let [(first, first_at), (second, second_at)] = wanted[..] else {
        panic!("two issues' texts: {wanted:?}");
    };
    let mut state = projection.state().clone();
    // another place, its own sum: refused as another place's
    assert!(
        state
            .read_texts(first, second_at, &at(&file, second_at))
            .is_err()
    );
    assert!(
        state
            .read_texts(second, first_at, &at(&file, first_at))
            .is_err()
    );
    assert!(!state.issue(first).unwrap().texts().here());
}

#[test]
fn an_issue_many_records_are_about_has_its_texts_read_once() {
    let (projection, file) = with_texts();
    let (two, five) = (
        EventId::parse(&id(2)).unwrap(),
        EventId::parse(&id(5)).unwrap(),
    );
    let wanted = projection.state().wanted_texts([five, two, two, five, two]);
    assert_eq!(wanted.len(), 2, "{wanted:?}");
    assert!(wanted.windows(2).all(|pair| pair[0].1.at < pair[1].1.at));
    let mut state = projection.state().clone();
    for (uid, place) in &wanted {
        state.read_texts(*uid, *place, &at(&file, *place)).unwrap();
    }
    assert!(state.wanted_texts([two, five]).is_empty());
    // texts read in already are not read again: no bytes are asked of them
    let (_, place) = wanted[0];
    assert!(state.read_texts(two, place, &[]).is_ok());
}
