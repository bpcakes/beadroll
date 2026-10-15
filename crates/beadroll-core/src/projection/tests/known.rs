//! What a projection's parts know of its events (roll-lqjj.35): its state part, read alone as a
//! command that only reads reads it, only who wrote the events behind a register with several
//! entries, the only ones its conflicts name, and none of their causes; its index every event the
//! state names, with its causes, machine and actor, which applying more events reads.

use std::collections::HashSet;

use crate::event::Field;
use crate::id::EventId;
use crate::state::State;

use super::super::{Projection, asked, decode_state, named, project};
use super::hostile::{HERE, base, event, fields, id, journal, record};

/// Whether `decoded`, read back from what `projection` encoded, knows every event the state names
/// and the frontier, in the journal's order, each with its causes, machine and actor as
/// `projection` has them.
pub(super) fn knows_alike(decoded: &Projection, projection: &Projection) -> bool {
    let read = &decoded.state.knowledge.events;
    let held = &projection.state.knowledge.events;
    let mut kept = named(projection.state());
    kept.extend(projection.heads.iter().copied());
    let order = held.keys().filter(|id| kept.contains(*id));
    read.keys().eq(order)
        && (read.iter()).all(|(id, event)| {
            held.get(id).is_some_and(|was| {
                event.deps() == was.deps()
                    && event.replica() == was.replica()
                    && event.actor() == was.actor()
            })
        })
}

/// Whether `state`, read alone from what `projection` encoded, knows of its events exactly those
/// behind a register with several entries, each written by whom `projection` says and without
/// its causes.
pub(super) fn knows_writers_alone(state: &State, projection: &Projection) -> bool {
    let writers: HashSet<EventId> = (asked(projection.state()).into_iter())
        .filter(|id| projection.state().holds(*id))
        .collect();
    state.events().collect::<HashSet<_>>() == writers
        && (state.knowledge.events.iter()).all(|(id, event)| {
            event.deps().is_empty() && state.who(*id) == projection.state().who(*id)
        })
}

#[test]
fn a_state_read_alone_knows_who_wrote_what_its_conflicts_name_and_the_index_every_cause() {
    const THERE: &str = "0000bb02";
    // two machines retitle issue 2, neither seeing the other: its title register holds both
    let mut records = base();
    let (theirs, ours) = (fields("theirs"), fields("ours"));
    records.push(record(event(4, THERE, &[3], "set", 2, theirs), THERE, true));
    records.push(record(event(5, HERE, &[3], "set", 2, ours), HERE, true));
    let projection = project(journal(&records)).unwrap();
    let (encoded, _) = projection.encode(0);
    let state = decode_state(&encoded.state).unwrap();
    let both: Vec<EventId> = [4, 5].map(|n| EventId::parse(&id(n)).unwrap()).into();
    assert_eq!(state.events().collect::<Vec<_>>(), both);
    assert!(knows_writers_alone(&state, &projection));
    let issue = |state: &State| state.issues().next().map(|issue| state.conflicts(issue));
    let conflicts = issue(&state).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].0, Field::Title);
    assert_eq!(
        format!("{conflicts:?}"),
        format!("{:?}", issue(projection.state()).unwrap())
    );
    // the index holds what the state names, the create and the two titles, with their causes
    let decoded = Projection::decode(&encoded).unwrap();
    assert!(knows_alike(&decoded, &projection));
    assert_eq!(decoded.state.knowledge.events.len(), 3);
    let causes = |n| (decoded.state.knowledge.events.get(&both[n])).map(|event| event.deps());
    let three = EventId::parse(&id(3)).unwrap();
    assert_eq!(causes(0), Some(&[three][..]));
    assert_eq!(causes(1), Some(&[three][..]));
}
