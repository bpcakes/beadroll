//! The order replay applies events in (Python's `effective_deps` and `causal_order`): every event
//! after its causes, ties broken by id.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};

use crate::id::EventId;
use crate::journal::{Event, Journal};
use crate::json::Value;

/// What an event waits for. A quarantined id is inert anyway, so it waits only for causes that
/// exist: those its own journal's copy names (that copy is as trustworthy as the journal's other
/// events), or, lacking one, those every variant agrees on, so one bad variant cannot hold
/// everything after it.
pub fn effective_deps(journal: &Journal, event: &Event) -> Vec<EventId> {
    let Some(variants) = journal.quarantined().get(&event.id()) else {
        return event.deps().to_vec();
    };
    if !event.deps().is_empty() {
        return event
            .deps()
            .iter()
            .copied()
            .filter(|dep| journal.event(*dep).is_some())
            .collect();
    }
    let mut common: Option<BTreeSet<EventId>> = None;
    for variant in variants {
        let listed = variant
            .get("deps")
            .and_then(Value::as_array)
            .unwrap_or_default();
        let deps: BTreeSet<EventId> = listed
            .iter()
            .filter_map(|dep| dep.as_str().and_then(EventId::parse))
            .filter(|dep| journal.event(*dep).is_some())
            .collect();
        common = Some(match common {
            None => deps,
            Some(common) => common.intersection(&deps).copied().collect(),
        });
    }
    common.unwrap_or_default().into_iter().collect()
}

/// Events with all their causes first, ties broken by id; and the rest, which wait for missing
/// causes, in journal order. A quarantined id whose remaining causes loop back onto it is released:
/// it applies nothing.
pub fn causal_order(journal: &Journal) -> (Vec<EventId>, Vec<EventId>) {
    let events = journal.events();
    let mut waiting = vec![0i64; events.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); events.len()];
    let mut ready = BinaryHeap::new();
    for (index, event) in events.values().enumerate() {
        let deps = effective_deps(journal, event);
        if let Some(count) = waiting.get_mut(index) {
            *count = i64::try_from(deps.len()).unwrap_or(i64::MAX);
        }
        for dep in &deps {
            // A cause this journal does not hold never applies; the event waits for good.
            if let Some(slot) = events
                .get_index_of(dep)
                .and_then(|at| dependents.get_mut(at))
            {
                slot.push(index);
            }
        }
        if deps.is_empty() {
            ready.push(Reverse(event.id()));
        }
    }
    let mut ordered = Vec::new();
    let mut applied = vec![false; events.len()];
    loop {
        while let Some(Reverse(id)) = ready.pop() {
            let Some(index) = events.get_index_of(&id) else {
                continue;
            };
            ordered.push(id);
            if let Some(flag) = applied.get_mut(index) {
                *flag = true;
            }
            for &child in dependents.get(index).map(Vec::as_slice).unwrap_or_default() {
                let Some(count) = waiting.get_mut(child) else {
                    continue;
                };
                *count -= 1;
                if *count == 0
                    && let Some((child_id, _)) = events.get_index(child)
                {
                    ready.push(Reverse(*child_id));
                }
            }
        }
        let stuck: Vec<usize> = journal
            .quarantined()
            .keys()
            .filter_map(|id| events.get_index_of(id))
            .filter(|at| {
                !applied.get(*at).copied().unwrap_or(true)
                    && waiting.get(*at).is_some_and(|w| *w > 0)
            })
            .collect();
        if stuck.is_empty() {
            break;
        }
        for at in stuck {
            if let (Some(count), Some((id, _))) = (waiting.get_mut(at), events.get_index(at)) {
                *count = 0;
                ready.push(Reverse(*id));
            }
        }
    }
    let pending = events
        .keys()
        .enumerate()
        .filter(|(at, _)| !applied.get(*at).copied().unwrap_or(false))
        .map(|(_, id)| *id)
        .collect();
    (ordered, pending)
}
