//! Replay: a journal folded into current state (Python's `replay`). Any machine holding the same
//! events gets the same result. Three phases, as in Python: what every event had seen and which
//! cannot apply at all; which claims a takeover supersedes; then each event applied in causal order.

mod apply;

use std::collections::{HashMap, HashSet};

use crate::event::{FORMAT, payload_problem};
use crate::id::EventId;
use crate::journal::Journal;
use crate::json::Value;
use crate::op::{Op, PyInt};
use crate::order::{causal_order, effective_deps};
use crate::state::{Bits, Integrity, KeySpace, Knowledge, Seen, State, actor_label};
use crate::text::str_of;
use crate::time::event_time;

/// Folds `journal` into current state. Fails only as Python's replay does, when a renamed copy can
/// find no free key (`KEY_SPACE`).
pub fn replay(mut journal: Journal) -> Result<State, KeySpace> {
    journal.settle();
    let (ordered, pending) = causal_order(&journal);
    let mut st = State::new(journal);
    st.pending = pending;
    for entry in &st.knowledge.journal.invalid {
        st.integrity.push(Integrity {
            event: entry.event.clone(),
            issue: None,
            problem: entry.problem.clone(),
            origin: Some(entry.origin.clone()),
        });
    }
    let mut inert = observe(&mut st.knowledge, &ordered);
    let superseded = settle_takeovers(&st.knowledge, &ordered, &mut inert);
    let knowledge = std::mem::take(&mut st.knowledge);
    let applied = apply_all(&mut st, &knowledge, &ordered, &inert, &superseded);
    st.knowledge = knowledge;
    applied?;
    for issue in st.issues.values_mut() {
        issue.labels = issue
            .label_tags
            .iter()
            .filter(|(_, tags)| !tags.is_empty())
            .map(|(label, _)| label.clone())
            .collect();
        issue.deps = issue
            .dep_tags
            .iter()
            .filter_map(|(edge, tags)| {
                tags.first_key_value()
                    .map(|(_, meta)| (edge.clone(), meta.clone()))
            })
            .collect();
    }
    st.index_children();
    Ok(st)
}

/// Phase 1: what every event had seen, and which events cannot be applied at all, each with the
/// integrity text it is reported with (empty: reported when it was loaded).
fn observe(k: &mut Knowledge, ordered: &[EventId]) -> HashMap<EventId, String> {
    let Knowledge {
        journal,
        replicas,
        seq,
        seen: seen_by,
        own_anc,
        bit,
        chain_breaks,
    } = k;
    let placeholder = |op: &str| matches!(op, "quarantined" | "invalid");
    // Each machine's positions that hold an applicable event.
    let applicable: Vec<Vec<usize>> = journal
        .order()
        .values()
        .map(|ids| {
            ids.iter()
                .enumerate()
                .filter(|(_, id)| {
                    journal
                        .event(**id)
                        .is_some_and(|e| !placeholder(e.op_name()))
                })
                .map(|(number, _)| number)
                .collect()
        })
        .collect();
    // The last position before `number` in a machine's journal that holds an applicable event.
    let predecessor = |replica: usize, number: usize| -> i64 {
        let positions = applicable
            .get(replica)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let at = positions.partition_point(|position| *position < number);
        at.checked_sub(1)
            .and_then(|before| positions.get(before))
            .map_or(-1, |position| i64::try_from(*position).unwrap_or(i64::MAX))
    };
    let mut inert = HashMap::new();
    for &id in ordered {
        let Some(event) = journal.event(id) else {
            continue;
        };
        let mut seen = Seen::default();
        let mut anc = Bits::default();
        for dep in effective_deps(journal, event) {
            let Some(dep_seen) = seen_by.get(&dep) else {
                continue; // a released quarantined id may name a cause that never applied
            };
            seen.merge(dep_seen);
            if let Some(dep_anc) = own_anc.get(&dep) {
                anc.union(dep_anc);
            }
            if !seq.contains_key(&dep) {
                let next = bit.len();
                anc.insert(*bit.entry(dep).or_insert(next));
            }
        }
        own_anc.insert(id, anc);
        if let Some(&(replica, number)) = seq.get(&id) {
            if seen.get(replica) < predecessor(replica, number) {
                // Not a single writer's log: the event does not build on its predecessor. Report it
                // and do not pretend it saw what came before (a copied machine identity looks like
                // this). A placeholder that fails this must not lend its position to what follows.
                let name = replicas.get(replica).cloned().unwrap_or_default();
                if !placeholder(event.op_name()) {
                    let problem = format!(
                        "journal chain broken at position {number} of machine {name} (copied machine identity?)"
                    );
                    inert.insert(id, problem);
                }
                chain_breaks.entry(name).or_default().push(number);
                seen_by.insert(id, seen);
                continue;
            }
            seen.raise(replica, i64::try_from(number).unwrap_or(i64::MAX));
        }
        seen_by.insert(id, seen);
        let version = event.get("v").cloned().unwrap_or(Value::from(FORMAT));
        let reason = if journal.quarantined().contains_key(&id) {
            Some("one event id with different contents".to_owned())
        } else if event.op_name() == "invalid" {
            Some(String::new())
        } else if !same_int(&version, FORMAT) {
            Some(format!(
                "event format v{} is not one this version of bead reads",
                str_of(&version)
            ))
        } else {
            payload_problem(event.value())
        };
        if let Some(reason) = reason {
            inert.insert(id, reason);
        }
    }
    inert
}

/// Python's `value != n` for an integer field: `true` equals 1.
fn same_int(value: &Value, n: i64) -> bool {
    PyInt::read(value).is_some_and(|value| value == PyInt::from(n))
}

/// Phase 2: ownership bookkeeping, over applicable events only. A takeover is settled online, so it
/// outranks anything its predecessor published later, whichever of the two sorts first here. Returns
/// the claims superseded.
fn settle_takeovers(
    k: &Knowledge,
    ordered: &[EventId],
    inert: &mut HashMap<EventId, String>,
) -> HashSet<EventId> {
    let mut superseded = HashSet::new();
    for &id in ordered {
        let Some(event) = k.journal.event(id) else {
            continue;
        };
        if inert.contains_key(&id) || event.op_name() != "claim" {
            continue;
        }
        let Op::Claim(claiming) = event.op() else {
            continue;
        };
        let Some(takeover) = claiming.takeover.as_deref().filter(|t| !t.is_empty()) else {
            continue;
        };
        let previous = EventId::parse(takeover).and_then(|p| k.journal.event(p));
        match previous {
            Some(previous)
                if previous.op_name() == "claim" && previous.issue() == event.issue() =>
            {
                if !k.saw(id, previous.id()) {
                    inert.insert(id, "takeover of a claim its writer had not seen".into());
                } else if claiming.mode.as_deref() != Some("offline")
                    && !inert.contains_key(&previous.id())
                {
                    superseded.insert(previous.id());
                }
            }
            _ => {
                inert.insert(
                    id,
                    "takeover names a claim that is not a claim on this issue".into(),
                );
            }
        }
    }
    superseded
}

/// Phase 3: every event applied, or reported.
fn apply_all(
    st: &mut State,
    k: &Knowledge,
    ordered: &[EventId],
    inert: &HashMap<EventId, String>,
    superseded: &HashSet<EventId>,
) -> Result<(), KeySpace> {
    for &id in ordered {
        let Some(event) = k.journal.event(id) else {
            continue;
        };
        let uid = event.issue();
        if let Some(reason) = inert.get(&id) {
            if !reason.is_empty() {
                let issue = uid.map(|uid| st.key_of(uid).to_owned());
                st.integrity.push(Integrity {
                    event: Some(id.to_string()),
                    issue,
                    problem: reason.clone(),
                    origin: None,
                });
            }
            continue;
        }
        let actor = event.get("actor").filter(|actor| actor.truthy());
        let who = actor_label(actor, event.replica());
        let when = event_time(id.millis());
        let thread = actor
            .and_then(|actor| actor.get("thread"))
            .and_then(Value::as_str);
        st.last_by_thread
            .insert((event.replica().to_owned(), thread.map(str::to_owned)), id);
        let applied = apply::apply_event(st, k, event, &who, &when, superseded)?;
        let target = uid.and_then(EventId::parse);
        if let Some(issue) = target
            .filter(|_| applied)
            .and_then(|uid| st.issues.get_mut(&uid))
        {
            if !event.data().get("imported").is_some_and(Value::truthy) {
                issue.set(
                    crate::event::Field::UpdatedAt,
                    Some(Value::from(when.as_str())),
                );
            }
            if let Some(place) = event.get("where").filter(|place| place.truthy()) {
                issue.where_ = Some(place.clone());
            }
        }
    }
    Ok(())
}
