//! What `bead recap` shows of a journal, and what it has seen once it has (roll-dih8.2): the
//! events not below the frontier it kept the last time, in causal order, and the frontier of every
//! event whose causes the journal holds.
//!
//! A recap shows only events whose causes are all here, so what it has shown is always everything
//! below a frontier, and the frontier is all it keeps, however long the history: a late sync, a
//! machine whose clock is behind, or a tracker first seen today cannot hide an event, since none
//! of that changes what is below. Below means reached from the frontier through the causes each
//! event names, with one exception. A quarantined id's copies disagree, so causal order may place
//! it before a cause one of them names has arrived: the walk stops at it, and the frontier keeps
//! what it was built on instead. A cause that arrives late is then shown, never taken for seen; the
//! price is that an id found quarantined after it was shown lets what it was built on show again,
//! once.

use crate::id::EventId;
use crate::journal::Journal;
use crate::order::causal_order;

/// Which of a journal's events a recap shows.
#[derive(Clone, Copy, Debug)]
pub enum Window<'a> {
    /// Those not below `frontier`, which [`unseen`] gave the last time.
    After(&'a [EventId]),
    /// Those written at or after `millis`, by the time their ids carry: what a tracker shows the
    /// first time, rather than its whole history.
    Since(u64),
    /// Every one.
    All,
}

/// What a recap shows of a journal, and what it has seen once it has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unseen {
    /// The events to show, in causal order, `init` left out.
    pub events: Vec<EventId>,
    /// How many events written before a [`Window::Since`] were left out, `init` not counted.
    pub earlier: u64,
    /// The frontier of every event whose causes the journal holds, those shown and those left out:
    /// what a recap keeps once it has shown them.
    pub frontier: Vec<EventId>,
    /// How many events that frontier covers. Of two frontiers taken of one journal as it grows,
    /// the one that covers more saw everything the other did.
    pub covered: u64,
}

/// What a recap of `journal`, a settled one, shows through `window`, and what it has seen then.
pub fn unseen(journal: &Journal, window: Window<'_>) -> Unseen {
    let (ordered, _) = causal_order(journal);
    let seen = match window {
        Window::After(frontier) => below(journal, frontier),
        Window::Since(_) | Window::All => Vec::new(),
    };
    let mut events = Vec::new();
    let mut earlier = 0;
    for id in &ordered {
        let Some((at, _, event)) = journal.events().get_full(id) else {
            continue;
        };
        if seen.get(at).copied().unwrap_or(false) || event.op_name() == "init" {
            continue;
        }
        match window {
            Window::Since(millis) if id.millis() < millis => earlier += 1,
            _ => events.push(*id),
        }
    }
    Unseen {
        events,
        earlier,
        frontier: frontier(journal, &ordered),
        covered: ordered.len() as u64,
    }
}

/// Which of the journal's events, by index, are below `frontier`: reached from it through the
/// causes each event names, the walk stopping at quarantined ids. An id of `frontier` the journal
/// does not hold is passed over.
fn below(journal: &Journal, frontier: &[EventId]) -> Vec<bool> {
    let events = journal.events();
    let mut seen = vec![false; events.len()];
    let mut next: Vec<usize> = (frontier.iter())
        .filter_map(|id| events.get_index_of(id))
        .collect();
    while let Some(at) = next.pop() {
        match seen.get_mut(at) {
            Some(flag) if !*flag => *flag = true,
            _ => continue,
        }
        let Some((id, event)) = events.get_index(at) else {
            continue;
        };
        if !journal.quarantined().contains_key(id) {
            next.extend(
                event
                    .deps()
                    .iter()
                    .filter_map(|dep| events.get_index_of(dep)),
            );
        }
    }
    seen
}

/// The frontier of `ordered`, events in causal order: those none of the others builds on, the
/// causes a quarantined id names not counted as built on. Sorted.
fn frontier(journal: &Journal, ordered: &[EventId]) -> Vec<EventId> {
    let events = journal.events();
    let mut built_on = vec![false; events.len()];
    for id in ordered {
        let Some(event) = events.get(id) else {
            continue;
        };
        if journal.quarantined().contains_key(id) {
            continue;
        }
        for at in event
            .deps()
            .iter()
            .filter_map(|dep| events.get_index_of(dep))
        {
            if let Some(flag) = built_on.get_mut(at) {
                *flag = true;
            }
        }
    }
    let mut heads: Vec<EventId> = (ordered.iter().copied())
        .filter(|id| {
            let at = events.get_index_of(id);
            at.is_some_and(|at| !built_on.get(at).copied().unwrap_or(true))
        })
        .collect();
    heads.sort_unstable();
    heads
}

#[cfg(test)]
mod tests;
