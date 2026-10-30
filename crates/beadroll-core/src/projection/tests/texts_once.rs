//! Each long text is held once (roll-lqjj.10): in its register, which the field shows, both in a
//! state and in the projection's text file; and the field shows what replay always showed.

use super::super::{Projection, Record, project};
use super::hostile::{HERE, event, id, journal, record};
use super::shown::replayed;
use crate::event::Field;
use crate::id::EventId;
use crate::journal::JournalOrder;
use crate::json::{Object, Value};
use crate::state::State;

const THERE: &str = "0000bb02";

/// Event `n` on `replica`, after `deps`, setting issue 2's description to `value`.
fn describe(n: u128, replica: &str, deps: &[u128], value: Value) -> Record {
    let mut fields = Object::new();
    fields.insert("description".into(), value);
    let mut data = Object::new();
    data.insert("fields".into(), Value::from(fields));
    record(
        event(n, replica, deps, "set", 2, data),
        replica,
        JournalOrder::Sequenced,
    )
}

/// An `init` and issue 2, created with `description` when given (null clears it).
fn created(description: Option<Value>) -> Vec<Record> {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from("described"));
    if let Some(description) = description {
        fields.insert("description".into(), description);
    }
    let mut create = Object::new();
    create.insert("key".into(), Value::from("pr-a1"));
    create.insert("fields".into(), Value::from(fields));
    vec![
        record(
            event(1, HERE, &[], "init", 1, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(2, HERE, &[1], "create", 2, create),
            HERE,
            JournalOrder::Sequenced,
        ),
    ]
}

/// Issue 2's description in `state`.
fn description(state: &State) -> Option<Value> {
    let uid = EventId::parse(&id(2)).unwrap();
    let issue = state.issue(uid).unwrap();
    issue.field(Field::Description).cloned()
}

/// The state of `records` as replay makes it, and as a projection of them read back from its
/// bytes, its texts read in, makes it.
fn both(records: &[Record]) -> (State, State) {
    let replayed = replayed(records);
    let (encoded, _) = project(journal(records)).unwrap().encode(0);
    let mut projection = Projection::decode(encoded.clone()).unwrap();
    let state = projection.state_mut();
    for (uid, place) in state.all_wanted_texts() {
        let at = usize::try_from(place.at).unwrap();
        let bytes = &encoded.texts[at..at + place.len as usize];
        state.read_texts(uid, place, bytes).unwrap();
    }
    (replayed, projection.state().clone())
}

#[test]
fn a_long_text_is_held_once_and_written_once() {
    let text = "d".repeat(100_000);
    let mut records = created(Some(Value::from(text.as_str())));
    let state = replayed(&records);
    let issue = state.issue(EventId::parse(&id(2)).unwrap()).unwrap();
    // in the register alone, which the field shows: an issue keeps no field's value beside it
    let register = issue.registers().get(&Field::Description).unwrap();
    assert_eq!(
        register.last().map(|(_, value)| value),
        issue.field(Field::Description)
    );
    assert_eq!(description(&state), Some(Value::from(text.as_str())));
    let (encoded, _) = project(journal(&records)).unwrap().encode(0);
    assert!(encoded.texts.len() < 101_000, "{}", encoded.texts.len());
    // another description replaces it: the texts hold the new one alone
    records.push(describe(3, HERE, &[2], Value::from("e".repeat(100_000))));
    let (encoded, _) = project(journal(&records)).unwrap().encode(0);
    assert!(encoded.texts.len() < 101_000, "{}", encoded.texts.len());
}

#[test]
fn a_long_field_shows_what_replay_always_showed() {
    // never given: absent
    let (replayed, projected) = both(&created(None));
    assert_eq!(description(&replayed), None);
    assert_eq!(description(&projected), None);
    // cleared by its create: null
    let mut records = created(Some(Value::Null));
    let (replayed, projected) = both(&records);
    assert_eq!(description(&replayed), Some(Value::Null));
    assert_eq!(description(&projected), Some(Value::Null));
    // set, then cleared by a set: absent
    records.push(describe(3, HERE, &[2], Value::from("x")));
    let (replayed, projected) = both(&records);
    assert_eq!(description(&replayed), Some(Value::from("x")));
    assert_eq!(description(&projected), Some(Value::from("x")));
    records.push(describe(4, HERE, &[3], Value::Null));
    let (replayed, projected) = both(&records);
    assert_eq!(description(&replayed), None);
    assert_eq!(description(&projected), None);
    // two writers unseen by each other: the larger id's value is shown, the other kept beside it
    records.push(describe(5, HERE, &[4], Value::from("here")));
    records.push(describe(6, THERE, &[4], Value::from("there")));
    let (replayed, projected) = both(&records);
    assert_eq!(description(&replayed), Some(Value::from("there")));
    assert_eq!(description(&projected), Some(Value::from("there")));
    let issue = replayed.issue(EventId::parse(&id(2)).unwrap()).unwrap();
    assert_eq!(replayed.conflicts(issue).len(), 1);
}
