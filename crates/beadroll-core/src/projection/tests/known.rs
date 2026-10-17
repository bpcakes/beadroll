//! What a projection's parts know of its events (roll-lqjj.35): its state part, read alone as a
//! command that only reads reads it, only who wrote the events behind a register with several
//! entries, the only ones its conflicts name, and none of their causes; its index every event the
//! state names, with its causes, machine and actor, which applying more events reads.

use std::collections::HashSet;

use crate::event::Field;
use crate::id::EventId;
use crate::json::{Object, Value};
use crate::state::State;

use super::super::{Extended, Projection, asked, decode_state, named, project};
use super::hostile::{HERE, base, event, fields, id, journal, record};
use super::read_in;
use super::shown::replayed;

/// The machine that writes beside this one in the tests below.
const THERE: &str = "0000bb02";

/// Whether `decoded`, read back from what `projection` encoded, knows every event the state names
/// and the frontier, in the journal's order, each with its causes, machine and actor as
/// `projection` has them: those alone when what every event had seen was `compacted`, and every
/// event `projection` knows otherwise (roll-lqjj.43).
pub(super) fn knows_alike(decoded: &Projection, projection: &Projection, compacted: bool) -> bool {
    let read = &decoded.state.knowledge.events;
    let held = &projection.state.knowledge.events;
    let mut kept = named(projection.state());
    kept.extend(projection.heads.iter().copied());
    let order = held.keys().filter(|id| !compacted || kept.contains(*id));
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
    assert!(knows_alike(&decoded, &projection, true));
    assert_eq!(decoded.state.knowledge.events.len(), 3);
    let causes = |n| (decoded.state.knowledge.events.get(&both[n])).map(|event| event.deps());
    let three = EventId::parse(&id(3)).unwrap();
    assert_eq!(causes(0), Some(&[three][..]));
    assert_eq!(causes(1), Some(&[three][..]));
}

#[test]
fn a_state_read_alone_knows_who_wrote_a_long_field_whose_texts_were_not_read_in() {
    // two machines set issue 2's design, neither seeing the other; read back, the projection is
    // extended by a title set, which needs no texts, so the design's writers are named from the
    // texts not read in when it is encoded again
    let mut records = base();
    for (n, folder, design) in [(4, THERE, "theirs"), (5, HERE, "ours")] {
        let mut set = Object::new();
        set.insert("design".into(), Value::from(design));
        let mut data = Object::new();
        data.insert("fields".into(), Value::from(set));
        records.push(record(event(n, folder, &[3], "set", 2, data), folder, true));
    }
    let (encoded, _) = project(journal(&records)).unwrap().encode(0);
    let mut file = encoded.texts.clone();
    let retitle = record(event(6, HERE, &[4, 5], "set", 2, fields("t")), HERE, true);
    records.push(retitle.clone());
    let read = Projection::decode(&encoded).unwrap();
    let Extended::Applied(extended) = read.extend(vec![retitle]).unwrap() else {
        panic!("a title set needs no texts");
    };
    assert!(extended.state().issues().all(|issue| !issue.texts().here()));
    let (encoded, _) = extended.encode(file.len() as u64);
    file.extend_from_slice(&encoded.texts);
    let mut state = decode_state(&encoded.state).unwrap();
    read_in(&mut state, &file, None);
    let both: Vec<EventId> = [4, 5].map(|n| EventId::parse(&id(n)).unwrap()).into();
    assert_eq!(state.events().collect::<Vec<_>>(), both);
    let issue = |state: &State| state.issues().next().map(|issue| state.conflicts(issue));
    let conflicts = issue(&state).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].0, Field::Design);
    let by: Vec<&str> = conflicts[0].1.iter().map(|each| each.by.as_str()).collect();
    assert!(by.iter().all(|by| !by.is_empty()), "{by:?}");
    let replayed = replayed(&records);
    assert_eq!(
        format!("{conflicts:?}"),
        format!("{:?}", issue(&replayed).unwrap())
    );
}
