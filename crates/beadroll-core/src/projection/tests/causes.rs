//! What a new event builds on (`Projection::causes_for`): the frontier, and its machine's last
//! applicable event where the frontier's clocks do not reach it, as replay judges a journal chain.

use super::super::project;
use super::awaiting::{THERE, at};
use super::hostile::{HERE, base, event, id, journal, record};
use crate::id::EventId;
use crate::journal::JournalOrder;
use crate::json::{Object, Value};

/// The event numbered `n`, by id.
fn event_id(n: u128) -> EventId {
    EventId::parse(&id(n)).expect("an id")
}

/// A comment saying `text` on the issue numbered 2.
fn comment(text: &str) -> Object {
    let mut data = Object::new();
    data.insert("text".into(), Value::from(text));
    data
}

/// A machine's last record that is a placeholder (malformed, and waiting on itself) is no event
/// its next one must build on: replay judges the chain by the last applicable one, which the
/// frontier holds, and a write naming the placeholder waited for ever (AT100).
#[test]
fn a_placeholder_last_in_the_journal_is_not_built_on() {
    let mut records = base();
    // a record whose `op` is no text, naming itself: malformed, its cause never applied
    let mut malformed = event(4, HERE, &[4], "set", 2, Object::new());
    if let Value::Object(members) = &mut malformed {
        members.insert("op".into(), Value::from(7));
    }
    records.push(record(malformed, HERE, JournalOrder::Sequenced));
    let projection = project(journal(&records)).expect("a projection");
    let causes = projection.causes_for(HERE);
    assert_eq!(causes.ids, vec![event_id(3)], "{causes:?}");
    assert!(!causes.waits, "{causes:?}");
}

/// A machine's last applicable event, pending on a cause no journal holds, named by a foreign
/// head whose id was found with two contents: replay may release that head without the event in
/// its clock, so that the head names it without reaching it. A write names the event too, and
/// waits with it, where one taken as building on it through the head broke its chain (AT100).
#[test]
fn a_last_event_a_released_head_names_without_reaching_is_built_on() {
    let mut records = base();
    records.push(at(4, HERE, &[3, 99], "comment", 2, comment("waits")));
    records.push(at(5, THERE, &[4], "comment", 2, comment("one content")));
    records.push(at(5, THERE, &[4], "comment", 2, comment("another")));
    let projection = project(journal(&records)).expect("a projection");
    let causes = projection.causes_for(HERE);
    assert!(causes.ids.contains(&event_id(4)), "{causes:?}");
    assert!(causes.waits, "{causes:?}");
}

/// A write that finds its machine's last event on the frontier, or in the past of an event on it,
/// names the frontier alone.
#[test]
fn a_last_event_the_frontier_reaches_adds_no_cause() {
    let mut records = base();
    records.push(at(4, THERE, &[3], "comment", 2, comment("built on it")));
    let projection = project(journal(&records)).expect("a projection");
    assert_eq!(projection.causes_for(HERE).ids, vec![event_id(4)]);
    assert_eq!(projection.causes_for(THERE).ids, vec![event_id(4)]);
}
