//! Replay: a journal folded into current state (Python's `replay`). Any machine holding the same
//! events gets the same result. Three phases, as in Python: what every event had seen and which
//! cannot apply at all; which claims a takeover supersedes; then each event applied in causal order.
//! Events are taken by their index in the journal throughout, so that no phase looks an id up again.
//!
//! Replay takes its journal apart (roll-dih8.11): the state keeps of each event only what `saw` and
//! `who` read, its causes, machine and actor, and lets go of what an event applies once it has
//! applied it, so that a journal and its state are not both held whole.

mod apply;

pub(crate) use apply::Names;

use std::collections::{HashMap, HashSet};

use crate::id::EventId;
use crate::journal::{Journal, Parts, Rest};
use crate::json::Value;
use crate::op::Op;
use crate::order::Order;
use crate::state::{Integrity, KeySpace, Knowledge, Place, State, actor_label, raise, reached};
use crate::time::event_time;

/// Folds `journal` into current state. Fails only as Python's replay does, when a renamed copy can
/// find no free key (`KEY_SPACE`).
pub fn replay(journal: Journal) -> Result<State, KeySpace> {
    Ok(replayed(journal)?.0)
}

/// [`replay`], with the claims takeovers superseded: a projection applies the events that come
/// after with them.
pub(crate) fn replayed(mut journal: Journal) -> Result<(State, HashSet<EventId>), KeySpace> {
    journal.settle();
    let mut order = Order::of(&journal);
    let Parts {
        mut events,
        order: chains,
        quarantined,
        invalid,
    } = journal.into_parts();
    // What each event does, taken from it by index: the state keeps the rest of each event.
    let mut applying: Vec<Option<Box<Rest>>> = (events.values_mut())
        .map(|event| Some(event.take_rest()))
        .collect();
    let placeholder = |at: usize| {
        applying
            .get(at)
            .and_then(Option::as_deref)
            .is_some_and(|what| matches!(what.op, Op::Quarantined | Op::Invalid))
    };
    let mut st = State::new(Knowledge::new(events, &chains, placeholder));
    drop(chains);
    st.pending = (order.pending.iter())
        .filter_map(|at| st.knowledge.events.get_index(*at))
        .map(|(id, _)| *id)
        .collect();
    for entry in invalid {
        st.integrity.push(Integrity {
            event: entry.event,
            issue: None,
            problem: entry.problem,
            origin: Some(entry.origin),
        });
    }
    let mut inert = observe(&mut st.knowledge, &order, &applying, &quarantined);
    order.forget_causes(); // what follows needs the order alone
    let superseded = settle_takeovers(&st.knowledge, &order.ordered, &applying, &mut inert);
    let knowledge = std::mem::take(&mut st.knowledge);
    let applied = apply_all(
        &mut st,
        &knowledge,
        &order.ordered,
        &mut applying,
        &inert,
        &superseded,
    );
    st.knowledge = knowledge;
    applied?;
    finish(&mut st);
    Ok((st, superseded))
}

/// Phase 1: what every event had seen, and which events cannot be applied at all, each with the
/// integrity text it is reported with (empty: reported when it was loaded), by event index; they
/// are few, so only they are held.
fn observe(
    k: &mut Knowledge,
    order: &Order,
    applying: &[Option<Box<Rest>>],
    quarantined: &HashSet<EventId>,
) -> HashMap<usize, String> {
    let mut inert = HashMap::new();
    let mut room = Room::default();
    for &at in &order.ordered {
        let Some(id) = k.events.get_index(at).map(|(id, _)| *id) else {
            continue;
        };
        let Some(what) = applying.get(at).and_then(Option::as_deref) else {
            continue;
        };
        let doubled = quarantined.contains(&id);
        if let Some(reason) = observe_one(k, at, order.causes(at), what, doubled, &mut room) {
            inert.insert(at, reason);
        }
    }
    inert
}

/// Room to merge clocks in, kept from one event to the next.
#[derive(Default)]
pub(crate) struct Room {
    clock: Vec<(u32, u32)>,
    spare: Vec<(u32, u32)>,
}

/// What the event at `at` had seen, from its `causes`, recorded; and whether it cannot be applied
/// at all, with the integrity text it is reported with (empty: reported when it was loaded).
/// `doubled`: its id was found with several contents.
pub(crate) fn observe_one(
    k: &mut Knowledge,
    at: usize,
    causes: &[u32],
    what: &Rest,
    doubled: bool,
    room: &mut Room,
) -> Option<String> {
    let Knowledge {
        events,
        replicas,
        applicable,
        ancestry,
        chain_breaks,
    } = k;
    let replica = events.get_index(at).map(|(_, event)| event.replica())?;
    let Room { clock, spare } = room;
    ancestry.gather(causes, clock, spare);
    if let Place::Sequenced(chain, number) = ancestry.place(at) {
        // The last position before this one in the machine's journal that holds an applicable event.
        let positions = applicable
            .get(chain as usize)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let before = positions.partition_point(|position| *position < number);
        let predecessor = (before.checked_sub(1))
            .and_then(|before| positions.get(before))
            .map_or(-1, |position| i64::from(*position));
        if reached(clock, chain).map_or(-1, i64::from) < predecessor {
            // Not a single writer's log: the event does not build on its predecessor. Report it
            // and do not pretend it saw what came before (a copied machine identity looks like
            // this). A placeholder that fails this must not lend its position to what follows.
            let name = replicas.get(chain as usize).cloned().unwrap_or_default();
            chain_breaks
                .entry(name.clone())
                .or_default()
                .push(number as usize);
            ancestry.record(at, clock, causes, replica);
            return (!matches!(what.op, Op::Quarantined | Op::Invalid)).then(|| {
                format!(
                    "journal chain broken at position {number} of machine {name} (copied machine identity?)"
                )
            });
        }
        raise(clock, chain, number);
    }
    ancestry.record(at, clock, causes, replica);
    if doubled {
        Some("one event id with different contents".to_owned())
    } else if matches!(what.op, Op::Invalid) {
        Some(String::new())
    } else {
        // its format or its payload, found when it was read
        what.flaw().map(str::to_owned)
    }
}

/// Phase 2: ownership bookkeeping, over applicable events only. A takeover is settled online, so it
/// outranks anything its predecessor published later, whichever of the two sorts first here. Returns
/// the claims superseded.
fn settle_takeovers(
    k: &Knowledge,
    ordered: &[usize],
    applying: &[Option<Box<Rest>>],
    inert: &mut HashMap<usize, String>,
) -> HashSet<EventId> {
    let what = |at: usize| applying.get(at).and_then(Option::as_deref);
    let mut superseded = HashSet::new();
    for &at in ordered {
        let Some(event) = what(at) else {
            continue;
        };
        if inert.contains_key(&at) {
            continue;
        }
        let Op::Claim(claiming) = &event.op else {
            continue;
        };
        let Some(takeover) = claiming.takeover.as_deref().filter(|t| !t.is_empty()) else {
            continue;
        };
        let previous = EventId::parse(takeover).and_then(|p| {
            let (before, previous, _) = k.events.get_full(&p)?;
            Some((before, *previous, what(before)?))
        });
        let refused = match previous {
            Some((before, previous, earlier))
                if matches!(earlier.op, Op::Claim(_)) && earlier.issue == event.issue =>
            {
                if !k.saw_at(at, previous) {
                    Some("takeover of a claim its writer had not seen")
                } else {
                    if claiming.mode.as_deref() != Some("offline") && !inert.contains_key(&before) {
                        superseded.insert(previous);
                    }
                    None
                }
            }
            _ => Some("takeover names a claim that is not a claim on this issue"),
        };
        if let Some(reason) = refused {
            inert.insert(at, reason.to_owned());
        }
    }
    superseded
}

/// Phase 3: every event applied, or reported.
fn apply_all(
    st: &mut State,
    k: &Knowledge,
    ordered: &[usize],
    applying: &mut [Option<Box<Rest>>],
    inert: &HashMap<usize, String>,
    superseded: &HashSet<EventId>,
) -> Result<(), KeySpace> {
    let names = Names::default();
    for &at in ordered {
        // applied once: what it does goes with this turn of the loop
        let Some(event) = applying.get_mut(at).and_then(Option::take) else {
            continue;
        };
        apply_one(st, k, at, &event, inert.get(&at), superseded, &names)?;
    }
    Ok(())
}

/// The event at `at`, doing `event`, applied; or reported, when it cannot apply (`inert`, with its
/// integrity text: empty when it was reported as it was loaded).
pub(crate) fn apply_one(
    st: &mut State,
    k: &Knowledge,
    at: usize,
    event: &Rest,
    inert: Option<&String>,
    superseded: &HashSet<EventId>,
    names: &Names,
) -> Result<(), KeySpace> {
    let Some((&id, trace)) = k.events.get_index(at) else {
        return Ok(());
    };
    let uid = event.issue.as_deref();
    if let Some(reason) = inert {
        if !reason.is_empty() {
            let issue = uid.map(|uid| st.key_of(uid).to_owned());
            st.integrity.push(Integrity {
                event: Some(id.to_string()),
                issue,
                problem: reason.clone(),
                origin: None,
            });
        }
        return Ok(());
    }
    let actor = trace.actor().filter(|actor| actor.truthy());
    let who = actor_label(actor, trace.replica());
    let when = event_time(id.millis());
    let thread = actor
        .and_then(|actor| actor.get("thread"))
        .and_then(Value::as_str);
    st.last_by_thread
        .insert((trace.replica().to_owned(), thread.map(str::to_owned)), id);
    let doing = apply::Doing {
        at,
        id,
        trace,
        what: event,
    };
    let applied = apply::apply_event(st, k, &doing, names, &who, &when, superseded)?;
    let target = uid.and_then(EventId::parse);
    if let Some(issue) = target
        .filter(|_| applied)
        .and_then(|uid| st.issues.get_mut(&uid))
    {
        if !event.imported {
            issue.set(
                crate::event::Field::UpdatedAt,
                Some(Value::from(when.as_str())),
            );
        }
        if let Some(place) = event.place.as_ref().filter(|place| place.truthy()) {
            issue.where_ = Some(std::sync::Arc::clone(place));
        }
    }
    Ok(())
}

/// What replay finishes with once every event has applied: each issue's labels, the labels whose
/// tags remain, and the parent-child index.
pub(crate) fn finish(st: &mut State) {
    for issue in st.issues.values_mut() {
        issue.labels = issue
            .label_tags
            .iter()
            .filter(|(_, tags)| !tags.is_empty())
            .map(|(label, _)| label.clone())
            .collect();
    }
    st.index_children();
}
