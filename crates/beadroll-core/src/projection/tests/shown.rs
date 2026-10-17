//! What every field of every issue shows (roll-lqjj.41), against a model of it made from the
//! issue's registers, the create that made it and the order replay applied the events in, which is
//! what replay showed when it copied each write's value out of its register into the issue: a
//! register's entry of the largest id (absent when that entry is a later write's null), the
//! members of the status register's (status, closed_at, close_reason), and what the create gave or
//! defaulted to beside them. Written before short fields were shown from their registers, and
//! green on the code that copied them out; checked on every history the differential tests replay,
//! and on the journals of two stores bead built ([`FIXTURES`]), whose fields are also compared with
//! what bead showed of them when they were recorded, by the code that copied them out.

use std::collections::HashMap;

use crate::event::Field;
use crate::id::EventId;
use crate::journal::Journal;
use crate::json::{Format, Value, from_text, write};
use crate::op::Op;
use crate::order::causal_order;
use crate::replay::replay;
use crate::state::{Issue, State, actor_label, is_text};
use crate::time::event_time;

use super::super::Record;
use super::hostile::journal;

/// What a field shows, by the model: exactly one value or none, or for `updated_at`, which every
/// event that touches the issue writes and no register holds, one of the times it can have.
#[derive(Debug)]
enum Shown {
    Exactly(Option<Value>),
    OneOf(Vec<Value>),
}

/// Asserts that every field of every issue of `state`, replayed from `journal`, shows what the
/// model makes of it.
pub(super) fn agree(journal: &Journal, state: &State) {
    let mut journal = journal.clone();
    journal.settle();
    let (ordered, _) = causal_order(&journal);
    let place: HashMap<EventId, usize> = (ordered.iter().enumerate())
        .map(|(at, id)| (*id, at))
        .collect();
    let order = Order {
        ordered: &ordered,
        place: &place,
    };
    for issue in state.issues() {
        for field in Field::ALL {
            let shown = issue.field(field);
            match model(&journal, &order, issue, field) {
                Shown::Exactly(expected) => assert_eq!(
                    shown,
                    expected.as_ref(),
                    "{} {}: {:?}",
                    issue.key(),
                    field.name(),
                    issue.registers()
                ),
                Shown::OneOf(times) => assert!(
                    shown.is_some_and(|shown| times.contains(shown)),
                    "{} {}: {shown:?} is none of {times:?}",
                    issue.key(),
                    field.name()
                ),
            }
        }
    }
}

/// The events in the order replay applied them, and each one's place in it.
struct Order<'a> {
    ordered: &'a [EventId],
    place: &'a HashMap<EventId, usize>,
}

/// What `field` of `issue` shows by the model.
fn model(journal: &Journal, order: &Order<'_>, issue: &Issue, field: Field) -> Shown {
    let uid = issue.uid();
    let made = journal.event(uid).expect("the create");
    let Op::Create(create) = made.op() else {
        panic!("{} was not made by a create", issue.key());
    };
    let given = |field: Field| {
        (create.fields.iter())
            .find(|(given, _)| *given == field)
            .map(|(_, value)| value.clone())
    };
    let registers = issue.registers();
    // the event that wrote a register last: the one of its entries replay applied last
    let latest = |field: Field| {
        let register = registers.get(&field)?;
        register.iter().map(|(id, _)| order.place[id]).max()
    };
    let member = |name: &str| {
        let (_, value) = registers.get(&Field::Status)?.last()?;
        value.get(name).cloned()
    };
    let exactly = match field {
        Field::CreatedAt => given(field).or_else(|| Some(Value::from(event_time(uid.millis())))),
        Field::CreatedBy => given(field).or_else(|| {
            Some(if create.imported {
                Value::Null
            } else {
                let actor = made.actor().filter(|actor| actor.truthy());
                Value::from(actor_label(actor, made.replica()))
            })
        }),
        Field::UpdatedAt => {
            let created = given(field).unwrap_or_else(|| Value::from(event_time(uid.millis())));
            let mut times = vec![created];
            let about = uid.to_string();
            for event in journal.events().values() {
                if event.issue() == Some(about.as_str()) && !event.imported() {
                    times.push(Value::from(event_time(event.id().millis())));
                }
            }
            return Shown::OneOf(times);
        }
        Field::Status => member("status"),
        Field::ClosedAt | Field::CloseReason => {
            // a set may write either field, into a register of its own; a status written after it
            // shows the status register's member again
            let own = match (latest(field), latest(Field::Status)) {
                (Some(own), Some(status)) if own == status => {
                    let Op::Set(fields) = journal.events()[&order.ordered[own]].op() else {
                        panic!("only a set writes both");
                    };
                    let at = |wanted: Field| fields.iter().rposition(|(f, _)| *f == wanted);
                    at(field) > at(Field::Status)
                }
                (Some(own), status) => status.is_none_or(|status| own > status),
                (None, _) => false,
            };
            if own {
                let (_, value) = registers
                    .get(&field)
                    .and_then(|r| r.last())
                    .expect("a value");
                (*value != Value::Null).then(|| value.clone())
            } else {
                member(field.name())
            }
        }
        _ if is_text(field) => {
            let last = registers.get(&field).and_then(|r| r.last());
            last.filter(|(id, value)| **value != Value::Null || **id == uid)
                .map(|(_, value)| value.clone())
        }
        _ => match registers.get(&field) {
            // the create's own value, null included, until a write replaces it
            Some(register) if register.iter().all(|(id, _)| *id == uid) => {
                register.last().map(|(_, value)| value.clone())
            }
            Some(register) => (register.last())
                .map(|(_, value)| value.clone())
                .filter(|value| *value != Value::Null),
            None => match field {
                Field::Title => Some(Value::from("")),
                Field::Priority => Some(Value::from(2)),
                Field::IssueType => Some(Value::from("task")),
                _ => None,
            },
        },
    };
    Shown::Exactly(exactly)
}

/// The journals of two stores bead built, each line a record and the file it was in under the
/// store's `events/` (as `<file>\t<record>`), and what every field of every issue showed when
/// they were recorded (`<key>\t<field>\t<value>`, absent fields left out): the golden tracker of
/// `crates/beadroll/tests/cli/golden.rs` up to its conflicting titles, and a fleet of two machines
/// (a Beads export with closed issues imported, concurrent edits of short fields and of a status
/// synced into conflicts, an issue closed, reopened and closed again, a claim released and claimed
/// again, a close by its owner, an imported closed issue reopened and closed by its status).
pub(super) const FIXTURES: [(&str, &str, &str); 2] = [
    (
        "golden",
        include_str!("shown/golden.journal"),
        include_str!("shown/golden.fields"),
    ),
    (
        "fleet",
        include_str!("shown/fleet.journal"),
        include_str!("shown/fleet.fields"),
    ),
];

/// The journal of a fixture's lines.
pub(super) fn fixture(lines: &str) -> Journal {
    let mut journal = Journal::new();
    for line in lines.lines() {
        let (file, record) = line.split_once('\t').expect("a file and a record");
        let replica = file.split('/').nth(1).expect("a machine's folder");
        let record = from_text(record).expect("a record");
        journal.add(record, replica, file, file.ends_with(".jsonl"));
    }
    journal
}

/// What every field of every issue of `state` shows, as a fixture records it.
fn fields(state: &State) -> String {
    let mut out = String::new();
    for issue in state.issues() {
        for field in Field::ALL {
            if let Some(value) = issue.field(field) {
                let value = write(value, Format::COMPACT).expect("a value");
                out.push_str(&format!("{}\t{}\t{value}\n", issue.key(), field.name()));
            }
        }
    }
    out
}

#[test]
fn every_field_of_the_stores_bead_built_shows_what_it_showed_and_the_model_says() {
    for (name, journal, recorded) in FIXTURES {
        let state = replay(fixture(journal)).unwrap();
        assert!(state.issues().count() > 5, "{name}");
        agree(&fixture(journal), &state);
        let shown = fields(&state);
        assert!(
            shown == recorded,
            "{name} shows otherwise than it did:\n{shown}"
        );
    }
}

/// What replay makes of `records`, every field of every issue checked against the model.
pub(super) fn replayed(records: &[Record]) -> State {
    let state = replay(journal(records)).unwrap();
    agree(&journal(records), &state);
    state
}
