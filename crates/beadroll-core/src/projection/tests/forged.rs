//! Bytes no bead writes, as a buggy build or bytes forged with their sums could leave them, that a
//! copying write-back would otherwise make worse (the review of roll-lqjj.44): a state part that
//! holds an issue or a key twice, whose records a write-back copies by position, and a count of the
//! entries its table had when it was made anew beyond those it holds, which the write-back adds to.
//! Each is refused, as a part that cannot be read is, and the journal is replayed.

use crate::id::EventId;
use crate::json::{Object, Value};

use super::super::{Encoded, Projection, Record, decode_state, project};
use super::hostile::{HERE, event, fields, id, journal, record};

/// `init`, then issues 2, 4 and 6 (keys pr-a, pr-b and pr-c), each created after the one before.
fn three() -> Vec<Record> {
    let create = |n: u128, key: &str, after: u128| {
        let mut data = fields(&format!("issue {n}"));
        data.insert("key".into(), Value::from(key));
        record(event(n, HERE, &[after], "create", n, data), HERE, true)
    };
    vec![
        record(event(1, HERE, &[], "init", 1, Object::new()), HERE, true),
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
        index: encoded.index.clone(),
        texts: Vec::new(),
    };
    assert!(Projection::decode(whole).is_ok(), "as written");
}
