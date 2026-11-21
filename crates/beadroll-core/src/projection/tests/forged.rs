//! Bytes no bead writes, as a buggy build or bytes forged with their sums could leave them, that a
//! copying write-back would otherwise make worse (the review of roll-lqjj.44): a state part that
//! holds an issue or a key twice, whose records a write-back copies by position, and a count of the
//! entries its table had when it was made anew beyond those it holds, which the write-back adds to.
//! Each is refused, as a part that cannot be read is, and the journal is replayed.

use super::super::{Encoded, Projection, Record, decode_state, project};
use super::hostile::{HERE, event, fields, id, journal, record};
use crate::id::EventId;
use crate::journal::JournalOrder;
use crate::json::{Object, Value};

/// `init`, then issues 2, 4 and 6 (keys pr-a, pr-b and pr-c), each created after the one before.
fn three() -> Vec<Record> {
    let create = |n: u128, key: &str, after: u128| {
        let mut data = fields(&format!("issue {n}"));
        data.insert("key".into(), Value::from(key));
        record(
            event(n, HERE, &[after], "create", n, data),
            HERE,
            JournalOrder::Sequenced,
        )
    };
    vec![
        record(
            event(1, HERE, &[], "init", 1, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
        create(2, "pr-a", 1),
        create(4, "pr-b", 2),
        create(6, "pr-c", 4),
    ]
}

/// Where `needle` first is in `bytes`.
fn find(bytes: &[u8], needle: &[u8]) -> usize {
    (bytes.windows(needle.len()))
        .position(|window| window == needle)
        .unwrap()
}

/// The sixteen bytes of event `n`'s id.
fn uid(n: u128) -> [u8; 16] {
    EventId::parse(&id(n)).unwrap().as_u128().to_be_bytes()
}

/// `state` with the bytes `from..to` written again right after them, the count one byte before
/// `from` one more, and the table's place, the part's first eight bytes, moved past them.
fn twice(state: &[u8], from: usize, to: usize) -> Vec<u8> {
    let again = state[from..to].to_vec();
    let mut out = state.to_vec();
    assert_eq!(out[from - 1], 3, "the count, in one byte");
    out[from - 1] = 4;
    out.splice(to..to, again.iter().copied());
    let at = u64::from_le_bytes(out[..8].try_into().unwrap()) + again.len() as u64;
    out[..8].copy_from_slice(&at.to_le_bytes());
    out
}

/// Whether the projection with this state part is refused, alone and with its index.
fn refused(state: Vec<u8>, index: &[u8]) -> bool {
    let whole = Encoded {
        state: state.clone(),
        index: index.to_vec(),
        texts: Vec::new(),
    };
    decode_state(&state).is_err() && Projection::decode(whole).is_err()
}

#[test]
fn a_state_part_that_holds_an_issue_or_a_key_twice_is_refused() {
    let (encoded, _) = project(journal(&three())).unwrap().encode(0);
    let state = &encoded.state;
    // pr-a's record twice: [A, A, B, C]
    let (a, b) = (find(state, &uid(2)), find(state, &uid(4)));
    assert!(
        refused(twice(state, a, b), &encoded.index),
        "an issue twice"
    );
    // pr-a's key entry twice: its text, then its issue's id
    let mut entry = vec![4];
    entry.extend_from_slice(b"pr-a");
    entry.extend_from_slice(&uid(2));
    let at = find(state, &entry);
    assert!(
        refused(twice(state, at, at + entry.len()), &encoded.index),
        "a key twice"
    );
}

#[test]
fn a_count_of_entries_made_anew_beyond_the_table_is_refused() {
    let (encoded, _) = project(journal(&three())).unwrap().encode(0);
    // the count is the part's last number, after its table
    let mut state = encoded.state.clone();
    let last = state.pop().unwrap();
    assert!(last < 0x80, "in one byte");
    for fresh in [u64::from(last) + 1, u64::MAX / 2, u64::MAX] {
        let mut forged = state.clone();
        let mut n = fresh;
        while n >= 0x80 {
            forged.push((n as u8) | 0x80);
            n >>= 7;
        }
        forged.push(n as u8);
        let whole = Encoded {
            state: forged,
            index: encoded.index.clone(),
            texts: Vec::new(),
        };
        assert!(Projection::decode(whole).is_err(), "{fresh} entries");
    }
    let whole = Encoded {
        state: [&state[..], &[last]].concat(),
        index: encoded.index,
        texts: Vec::new(),
    };
    assert!(Projection::decode(whole).is_ok(), "as written");
}

/// An index part naming as a machine's last applicable event one that is not, an earlier event
/// its knowledge holds, or naming none for a machine whose journal holds one, is refused: a write would build on it, and replay set the write aside as a
/// broken chain where the frontier did not reach the last (AT100).
#[test]
fn an_index_naming_another_event_last_in_a_journal_is_refused() {
    let (encoded, _) = project(journal(&three())).unwrap().encode(0);
    let mut entry = vec![8];
    entry.extend_from_slice(HERE.as_bytes());
    entry.extend_from_slice(&uid(6));
    // the machine's greatest id, then its last applicable event: the second entry is the tail
    let first = find(&encoded.index, &entry);
    let tail = first + entry.len() + find(&encoded.index[first + entry.len()..], &entry);
    let mut index = encoded.index.clone();
    index[tail + 1 + HERE.len()..tail + entry.len()].copy_from_slice(&uid(4));
    let whole = |index: Vec<u8>| Encoded {
        state: encoded.state.clone(),
        index,
        texts: Vec::new(),
    };
    assert!(
        Projection::decode(whole(index)).is_err(),
        "another event last"
    );
    // no entry for the machine at all: its count, one byte before it, none
    let mut index = encoded.index.clone();
    assert_eq!(index[tail - 1], 1, "the count, in one byte");
    index[tail - 1] = 0;
    index.drain(tail..tail + entry.len());
    // the table's place, the part's first eight bytes, moved back past what was taken out
    let at = u64::from_le_bytes(index[..8].try_into().unwrap()) - entry.len() as u64;
    index[..8].copy_from_slice(&at.to_le_bytes());
    let refused = Projection::decode(whole(index))
        .err()
        .map(|error| format!("{error:?}"));
    assert!(
        refused
            .as_deref()
            .is_some_and(|error| error.contains("last applicable")),
        "no event last: {refused:?}"
    );
    assert!(
        Projection::decode(whole(encoded.index.clone())).is_ok(),
        "as written"
    );
}

/// A projection whose ids hold one the journal does not, as an index read back could, declines an
/// event naming it beside the frontier: replay leaves that event waiting for it, where the
/// projection, taking the id for a cause applied, applied the event without it (AT100).
#[test]
fn an_id_the_journal_does_not_hold_is_no_cause_an_extension_applies_after() {
    let mut projection = project(journal(&three())).unwrap();
    let stale = EventId::parse(&id(90)).unwrap();
    projection.ids.insert(stale);
    let mut data = Object::new();
    data.insert("text".into(), Value::from("on the frontier and a stale id"));
    let event = event(7, HERE, &[6, 90], "comment", 2, data);
    let batch = vec![record(event, HERE, JournalOrder::Sequenced)];
    let extended = projection.extend(batch).unwrap();
    assert!(
        matches!(extended, super::super::Extended::Replay),
        "the event waits for a cause no journal holds"
    );
}

/// A projection whose ids lack one its knowledge keeps a record of, as an index read back could,
/// declines an event naming it beside the frontier: the journal does not hold it, and replay leaves
/// the event waiting for it (AT100).
#[test]
fn a_record_kept_without_its_id_is_no_cause_an_extension_applies_after() {
    let mut projection = project(journal(&three())).unwrap();
    let held = EventId::parse(&id(2)).unwrap();
    assert!(projection.ids.remove(&held), "an issue's create, kept");
    let mut data = Object::new();
    data.insert("text".into(), Value::from("on the frontier and a create"));
    let event = event(7, HERE, &[6, 2], "comment", 2, data);
    let batch = vec![record(event, HERE, JournalOrder::Sequenced)];
    let extended = projection.extend(batch).unwrap();
    assert!(
        matches!(extended, super::super::Extended::Replay),
        "the event waits for a cause the journal does not hold"
    );
}

/// An index naming an event its ids lack, as a buggy build could write one, is refused: a
/// journal's last applicable event off the frontier, which a write builds on, would otherwise be
/// a cause no journal holds, and the write wait for it; and so with any event its knowledge keeps,
/// a machine's latest, or a claim superseded (AT100).
#[test]
fn an_index_naming_an_event_its_ids_lack_is_refused() {
    let mut records = three();
    // another machine's comment on the last create: the frontier, which does not hold that create
    let mut data = Object::new();
    data.insert("text".into(), Value::from("built on the last create"));
    let there = "0000bb02";
    let on = event(8, there, &[6], "comment", 2, data);
    records.push(record(on, there, JournalOrder::Sequenced));
    let projection = project(journal(&records)).unwrap();
    let tail = EventId::parse(&id(6)).unwrap();
    assert_eq!(projection.tails.get(HERE), Some(&tail));
    assert!(
        !projection.heads.contains(&tail),
        "the tail off the frontier"
    );
    let refused = |forged: Projection| {
        let (encoded, _) = forged.encode(0);
        let refused = Projection::decode(encoded).err();
        (refused.map(|error| format!("{error:?}")))
            .is_some_and(|error| error.contains("the journal holds"))
    };
    let lacking = |n: u128| {
        let mut forged = projection.clone();
        assert!(forged.ids.remove(&EventId::parse(&id(n)).unwrap()), "{n}");
        forged
    };
    assert!(refused(lacking(6)), "the tail");
    assert!(refused(lacking(8)), "the frontier");
    assert!(refused(lacking(2)), "a create the state names");
    let stale = EventId::parse(&id(90)).unwrap();
    let mut forged = projection.clone();
    forged.lasts.insert(HERE.to_owned(), stale);
    assert!(refused(forged), "a machine's latest");
    let mut forged = projection.clone();
    forged.superseded.insert(stale);
    assert!(refused(forged), "a claim superseded");
    let (encoded, _) = projection.encode(0);
    assert!(Projection::decode(encoded).is_ok(), "as written");
}

/// A state part read with an index from before an issue it holds was made, as only bytes forged
/// with their sums, or a buggy build, could pair (both parts are of one file, under one header's
/// sums), is refused: the issue's create, which a child's create names among its causes, would
/// otherwise be a parent no journal holds, and the child wait for it (AT100).
#[test]
fn a_state_naming_an_event_the_index_lacks_is_refused() {
    let records = three();
    let (before, _) = project(journal(&records[..3])).unwrap().encode(0);
    let (after, _) = project(journal(&records)).unwrap().encode(0);
    let paired = |state: &[u8], index: &[u8]| Encoded {
        state: state.to_vec(),
        index: index.to_vec(),
        texts: Vec::new(),
    };
    let refused = Projection::decode(paired(&after.state, &before.index))
        .err()
        .map(|error| format!("{error:?}"));
    assert!(
        refused
            .as_deref()
            .is_some_and(|error| error.contains("the journal holds")),
        "an issue whose create the index lacks: {refused:?}"
    );
    assert!(
        Projection::decode(paired(&after.state, &after.index)).is_ok(),
        "as written"
    );
}

/// A state part read with an index from before the cause an issue's create waited for came, as
/// only bytes forged with their sums, or a buggy build, could pair, is refused though the index
/// keeps that create: it was pending there, and the issue a parent replay of that journal never
/// made (AT100). The cause, a machine's `init`, adds nothing else the state names.
#[test]
fn a_state_naming_a_create_the_index_holds_pending_is_refused() {
    let mut data = fields("issue 4");
    data.insert("key".into(), Value::from("pr-a"));
    let there = "0000bb02";
    let records = vec![
        record(
            event(1, HERE, &[], "init", 1, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(4, HERE, &[1, 3], "create", 4, data),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(3, there, &[], "init", 3, Object::new()),
            there,
            JournalOrder::Sequenced,
        ),
    ];
    let (before, _) = project(journal(&records[..2])).unwrap().encode(0);
    let after = project(journal(&records)).unwrap();
    assert_eq!(
        after.state().issues.len(),
        1,
        "the create applied once its cause came"
    );
    let (after, _) = after.encode(0);
    let paired = |state: &[u8], index: &[u8]| Encoded {
        state: state.to_vec(),
        index: index.to_vec(),
        texts: Vec::new(),
    };
    let refused = Projection::decode(paired(&after.state, &before.index))
        .err()
        .map(|error| format!("{error:?}"));
    assert!(
        refused
            .as_deref()
            .is_some_and(|error| error.contains("the journal holds")),
        "an issue whose create the index holds pending: {refused:?}"
    );
    assert!(
        Projection::decode(paired(&after.state, &after.index)).is_ok(),
        "as written"
    );
}
