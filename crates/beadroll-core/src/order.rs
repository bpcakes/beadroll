//! The order replay applies events in (Python's `effective_deps` and `causal_order`): every event
//! after its causes, ties broken by id.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};

use crate::id::EventId;
use crate::journal::{Event, Journal};

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
        let deps: BTreeSet<EventId> = (variant.deps().iter().copied())
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
    let order = Order::of(journal);
    let ids = |at: &[usize]| -> Vec<EventId> {
        at.iter()
            .filter_map(|at| journal.events().get_index(*at))
            .map(|(id, _)| *id)
            .collect()
    };
    (ids(&order.ordered), ids(&order.pending))
}

/// [`causal_order`] by each event's index in the journal's events, with the causes each event
/// waits for that the journal holds, so that replay need not look an id up again.
pub(crate) struct Order {
    /// The events in the order replay applies them.
    pub(crate) ordered: Vec<usize>,
    /// The events that wait for missing causes, in journal order.
    pub(crate) pending: Vec<usize>,
    /// Every event's effective causes that the journal holds, by index, as listed.
    causes: Lists,
}

/// A list of event indices for every event, kept flat.
struct Lists {
    /// Where each event's list begins in `items`; the last entry closes the last list.
    starts: Vec<usize>,
    items: Vec<usize>,
}

impl Lists {
    fn get(&self, at: usize) -> &[usize] {
        let start = self.starts.get(at).copied().unwrap_or(0);
        let end = self.starts.get(at + 1).copied().unwrap_or(start);
        self.items.get(start..end).unwrap_or_default()
    }

    /// The lists turned around: for every event, the events whose lists hold it, ascending, once
    /// for each time it is held.
    fn reversed(&self) -> Lists {
        let count = self.starts.len().saturating_sub(1);
        let mut starts = vec![0usize; count + 1];
        for item in &self.items {
            if let Some(slot) = starts.get_mut(item + 1) {
                *slot += 1;
            }
        }
        for at in 1..starts.len() {
            let before = starts.get(at - 1).copied().unwrap_or(0);
            if let Some(slot) = starts.get_mut(at) {
                *slot += before;
            }
        }
        let mut items = vec![0usize; self.items.len()];
        let mut filled = starts.clone();
        for at in 0..count {
            for item in self.get(at) {
                if let Some(slot) = filled.get_mut(*item)
                    && let Some(entry) = items.get_mut(*slot)
                {
                    *entry = at;
                    *slot += 1;
                }
            }
        }
        Lists { starts, items }
    }
}

impl Order {
    /// Lets go of what each event waits for: once replay has observed what every event had seen,
    /// it needs the order alone.
    pub(crate) fn forget_causes(&mut self) {
        self.causes = Lists {
            starts: Vec::new(),
            items: Vec::new(),
        };
    }

    /// The effective causes of the event at `at` that the journal holds, by index, as listed
    /// (a cause named twice is here twice).
    pub(crate) fn causes(&self, at: usize) -> &[usize] {
        self.causes.get(at)
    }

    /// Kahn's algorithm over the effective causes, the ready events in a heap by id, as Python's
    /// `heapq` takes them.
    pub(crate) fn of(journal: &Journal) -> Order {
        let events = journal.events();
        let count = events.len();
        // Signed, as Python's counts are: a released id's causes may still count down past zero.
        let mut waiting = vec![0i64; count];
        let mut causes = Lists {
            starts: Vec::with_capacity(count + 1),
            items: Vec::with_capacity(count),
        };
        let mut ready = BinaryHeap::new();
        for (index, event) in events.values().enumerate() {
            causes.starts.push(causes.items.len());
            let listed = journal.quarantined().contains_key(&event.id());
            let deps = if listed {
                effective_deps(journal, event)
            } else {
                Vec::new()
            };
            let deps = if listed { &deps[..] } else { event.deps() };
            if let Some(count) = waiting.get_mut(index) {
                // A cause this journal does not hold never applies: the event waits for good.
                *count = i64::try_from(deps.len()).unwrap_or(i64::MAX);
            }
            let held = deps.iter().filter_map(|dep| events.get_index_of(dep));
            causes.items.extend(held);
            if deps.is_empty() {
                ready.push(Reverse((event.id(), index)));
            }
        }
        causes.starts.push(causes.items.len());
        let dependents = causes.reversed();
        let mut ordered = Vec::with_capacity(count);
        let mut applied = vec![false; count];
        loop {
            while let Some(Reverse((_, index))) = ready.pop() {
                ordered.push(index);
                if let Some(flag) = applied.get_mut(index) {
                    *flag = true;
                }
                for &child in dependents.get(index) {
                    let Some(count) = waiting.get_mut(child) else {
                        continue;
                    };
                    *count -= 1;
                    if *count == 0
                        && let Some((child_id, _)) = events.get_index(child)
                    {
                        ready.push(Reverse((*child_id, child)));
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
                    ready.push(Reverse((*id, at)));
                }
            }
        }
        let pending = (0..count)
            .filter(|at| !applied.get(*at).copied().unwrap_or(false))
            .collect();
        Order {
            ordered,
            pending,
            causes,
        }
    }
}
