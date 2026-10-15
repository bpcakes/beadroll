//! Causal replay order: each event follows its causes, with ties broken by id.

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
    /// Whether a quarantined id is placed by less than all its causes: those it would wait for
    /// ([`effective_deps`]: its own copy's, or else those every copy names) include one the
    /// journal does not hold, which a record still to come can be, or its causes looped back onto
    /// it and it was released after every other event that could apply. Events that come after
    /// all of these can then be ordered before it.
    pub(crate) loose: bool,
    /// Every event's effective causes that the journal holds, by index, as listed.
    causes: Lists,
}

/// A list of event indices for every event, kept flat. A journal holds fewer than 2³² events (each
/// takes hundreds of bytes in memory), so an index is a `u32`.
struct Lists {
    /// Where each event's list begins in `items`; the last entry closes the last list.
    starts: Vec<usize>,
    items: Vec<u32>,
}

/// The index `at` as a list holds it.
fn small(at: usize) -> u32 {
    u32::try_from(at).unwrap_or(u32::MAX)
}

impl Lists {
    fn get(&self, at: usize) -> &[u32] {
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
            if let Some(slot) = starts.get_mut(*item as usize + 1) {
                *slot += 1;
            }
        }
        for at in 1..starts.len() {
            let before = starts.get(at - 1).copied().unwrap_or(0);
            if let Some(slot) = starts.get_mut(at) {
                *slot += before;
            }
        }
        let mut items = vec![0u32; self.items.len()];
        let mut filled = starts.clone();
        for at in 0..count {
            for item in self.get(at) {
                if let Some(slot) = filled.get_mut(*item as usize)
                    && let Some(entry) = items.get_mut(*slot)
                {
                    *entry = small(at);
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
    pub(crate) fn causes(&self, at: usize) -> &[u32] {
        self.causes.get(at)
    }

    /// Kahn's algorithm over effective causes, with ready events held in a heap ordered by id.
    pub(crate) fn of(journal: &Journal) -> Order {
        let events = journal.events();
        let count = events.len();
        // Signed counts are needed because a released id's cause count can fall below zero.
        let mut waiting = vec![0i64; count];
        let mut causes = Lists {
            starts: Vec::with_capacity(count + 1),
            items: Vec::with_capacity(count),
        };
        let mut ready = BinaryHeap::new();
        // what [`effective_deps`] lets go of: the causes its own copy names, or else those every
        // variant does, that the journal does not hold
        let unheld = |dep: &&EventId| !events.contains_key(*dep);
        let mut loose = (journal.quarantined().iter()).any(|(id, variants)| {
            match journal
                .event(*id)
                .map(Event::deps)
                .filter(|deps| !deps.is_empty())
            {
                Some(deps) => deps.iter().any(|dep| unheld(&dep)),
                None => (variants.first().map(Event::deps).unwrap_or_default().iter())
                    .filter(unheld)
                    .any(|dep| variants.iter().all(|variant| variant.deps().contains(dep))),
            }
        });
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
            causes.items.extend(held.map(small));
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
                    let child = child as usize;
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
            loose = true;
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
            loose,
            causes,
        }
    }
}
