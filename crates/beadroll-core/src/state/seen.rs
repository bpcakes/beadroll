//! What each event's writer had seen (Python's `State.seq`, `seen`, `own_anc` and `saw`), as one
//! clock per event.
//!
//! Python keeps two things per event: `seen`, the furthest position it had seen in each machine's
//! journal, and `own_anc`, a bitset of the unsequenced events (an attempt file's, which have no
//! position) in its past. The bitsets grow with the number of ownership events, so a long history
//! of claims and releases costs memory in proportion to events × ownership events. Here both are
//! positions in chains:
//!
//! - Each machine's journal is a chain, as in `seen`: chain `n` is the journal of `replicas[n]`.
//! - The unsequenced events are put into chains of their own as they are observed: an event joins a
//!   chain whose last event is already among its ancestors (its own machine's chain when it can), or
//!   starts one. So every event of such a chain is an ancestor of the next, and the ancestors an
//!   event has in a chain are always that chain's first few: one position says which (a chain
//!   decomposition, the classic reachability index). Writers build on everything they hold, so
//!   there are about as many chains as machines that write attempts.
//!
//! An event's clock holds, for every chain it has seen into, the furthest position it saw there,
//! sorted by chain; it is the clocks of its causes merged, its unsequenced causes added, and for a
//! sequenced event its own position, exactly as Python merges `seen` and ORs `own_anc`. Clocks live
//! in one arena; an event whose clock is one of its causes' shares it.
//!
//! A journal holds fewer than 2³² events (each takes kilobytes in memory), so chains and positions
//! are `u32`.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::id::EventId;
use crate::journal::Event;
use crate::state::actor_label;

/// Where an event is: a chain and its position there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// An unsequenced event not observed (yet): it has no place.
    Nowhere,
    /// `(chain, position)`: a position in a machine's journal, whose chain is that machine's.
    Sequenced(u32, u32),
    /// `(chain, position)`: an observed unsequenced event's position in the chain it joined.
    Linked(u32, u32),
}

/// A clock: the range of its entries in the arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    start: usize,
    end: usize,
}

/// The clock of an event not observed: it has seen nothing.
const UNOBSERVED: Span = Span {
    start: usize::MAX,
    end: usize::MAX,
};

/// What every event had seen, by event index.
#[derive(Clone, Debug, Default)]
pub(crate) struct Ancestry {
    /// Each event's place.
    places: Vec<Place>,
    /// Each event's clock.
    clocks: Vec<Span>,
    /// The clocks' entries: (chain, furthest position seen), each clock's sorted by chain.
    entries: Vec<(u32, u32)>,
    /// The number of machines' journals: the chains of unsequenced events are numbered after them.
    sequenced: u32,
    /// The length of each chain of unsequenced events.
    lengths: Vec<u32>,
    /// The chain each machine's unsequenced events last joined.
    last: HashMap<String, u32>,
}

/// `n` as a chain or position (a journal holds fewer than 2³² events).
fn small(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl Ancestry {
    /// The place of the event at `at`.
    pub(crate) fn place(&self, at: usize) -> Place {
        self.places.get(at).copied().unwrap_or(Place::Nowhere)
    }

    /// The entries of the clock of the event at `at`: none when it was not observed.
    fn clock(&self, at: usize) -> &[(u32, u32)] {
        let span = self.clocks.get(at).copied().unwrap_or(UNOBSERVED);
        self.entries.get(span.start..span.end).unwrap_or_default()
    }

    /// Whether the event at `at` has been observed.
    pub(crate) fn observed(&self, at: usize) -> bool {
        self.clocks.get(at).is_some_and(|span| *span != UNOBSERVED)
    }

    /// The furthest position the event at `at` saw in `chain`.
    fn reached(&self, at: usize, chain: u32) -> Option<u32> {
        reached(self.clock(at), chain)
    }

    /// Whether the event at `at` saw the event at `other` through its clock: `other`'s position in
    /// its chain is within what `at` saw of that chain.
    pub(crate) fn reaches(&self, at: usize, other: usize) -> bool {
        match self.place(other) {
            Place::Sequenced(chain, position) | Place::Linked(chain, position) => {
                self.reached(at, chain).is_some_and(|seen| seen >= position)
            }
            Place::Nowhere => false,
        }
    }

    /// Merges into `out` the clocks of the observed events among `causes`, each unsequenced one
    /// counted itself: what an event with these causes has seen. `spare` is room to merge in.
    pub(crate) fn gather(
        &self,
        causes: &[usize],
        out: &mut Vec<(u32, u32)>,
        spare: &mut Vec<(u32, u32)>,
    ) {
        out.clear();
        // A released quarantined id may name a cause that never applied.
        let mut observed = causes.iter().copied().filter(|cause| self.observed(*cause));
        let Some(first) = observed.next() else {
            return;
        };
        out.extend_from_slice(self.clock(first));
        if let Place::Linked(chain, position) = self.place(first) {
            raise(out, chain, position);
        }
        for cause in observed {
            merge(out, self.clock(cause), spare);
            if let Place::Linked(chain, position) = self.place(cause) {
                raise(out, chain, position);
            }
        }
    }

    /// Records `clock` as what the event at `at` had seen, sharing the clock of one of its `causes`
    /// when it is the same. An unsequenced event then joins a chain: its machine's (`replica`), if
    /// that chain's last event is among its ancestors; else the first chain whose last event is;
    /// else a chain of its own.
    pub(crate) fn record(
        &mut self,
        at: usize,
        clock: &[(u32, u32)],
        causes: &[usize],
        replica: &str,
    ) {
        let same = causes
            .iter()
            .find(|cause| self.observed(**cause) && self.clock(**cause) == clock)
            .and_then(|cause| self.clocks.get(*cause).copied());
        let span = same.unwrap_or_else(|| {
            let start = self.entries.len();
            self.entries.extend_from_slice(clock);
            Span {
                start,
                end: self.entries.len(),
            }
        });
        if let Some(slot) = self.clocks.get_mut(at) {
            *slot = span;
        }
        if self.place(at) != Place::Nowhere {
            return;
        }
        let found = self
            .last
            .get(replica)
            .copied()
            .filter(|chain| self.extends(clock, *chain))
            .or_else(|| {
                clock
                    .iter()
                    .map(|(chain, _)| *chain)
                    .find(|chain| *chain >= self.sequenced && self.extends(clock, *chain))
            });
        let chain = match found {
            Some(chain) => chain,
            None => {
                self.lengths.push(0);
                self.sequenced
                    .saturating_add(small(self.lengths.len().saturating_sub(1)))
            }
        };
        let Some(length) = chain
            .checked_sub(self.sequenced)
            .and_then(|n| self.lengths.get_mut(n as usize))
        else {
            return;
        };
        if let Some(place) = self.places.get_mut(at) {
            *place = Place::Linked(chain, *length);
        }
        *length = length.saturating_add(1);
        match self.last.get_mut(replica) {
            Some(last) => *last = chain,
            None => {
                self.last.insert(replica.to_owned(), chain);
            }
        }
    }

    /// Whether an event that saw `clock` can join `chain`: the chain's last event is among what it
    /// saw.
    fn extends(&self, clock: &[(u32, u32)], chain: u32) -> bool {
        let length = chain
            .checked_sub(self.sequenced)
            .and_then(|n| self.lengths.get(n as usize))
            .copied()
            .unwrap_or(0);
        length > 0 && reached(clock, chain) == Some(length - 1)
    }
}

/// The furthest position `clock` saw in `chain`.
pub(crate) fn reached(clock: &[(u32, u32)], chain: u32) -> Option<u32> {
    clock
        .binary_search_by_key(&chain, |(c, _)| *c)
        .ok()
        .and_then(|at| clock.get(at))
        .map(|(_, position)| *position)
}

/// Merges `other` into `clock`, both sorted by chain: the furthest of each chain's positions.
fn merge(clock: &mut Vec<(u32, u32)>, other: &[(u32, u32)], spare: &mut Vec<(u32, u32)>) {
    spare.clear();
    let (mut mine, mut theirs) = (clock.iter().peekable(), other.iter().peekable());
    loop {
        let next = match (mine.peek(), theirs.peek()) {
            (Some(a), Some(b)) if a.0 == b.0 => {
                let entry = (a.0, a.1.max(b.1));
                mine.next();
                theirs.next();
                entry
            }
            (Some(a), Some(b)) if a.0 < b.0 => {
                let entry = **a;
                mine.next();
                entry
            }
            (_, Some(b)) => {
                let entry = **b;
                theirs.next();
                entry
            }
            (Some(a), None) => {
                let entry = **a;
                mine.next();
                entry
            }
            (None, None) => break,
        };
        spare.push(next);
    }
    std::mem::swap(clock, spare);
}

/// Raises what `clock` saw of `chain` to `position`.
pub(crate) fn raise(clock: &mut Vec<(u32, u32)>, chain: u32, position: u32) {
    match clock.binary_search_by_key(&chain, |(c, _)| *c) {
        Ok(at) => {
            if let Some(entry) = clock.get_mut(at) {
                entry.1 = entry.1.max(position);
            }
        }
        Err(at) => clock.insert(at, (chain, position)),
    }
}

/// Every event replay took from its journal, by id, in the journal's order, and what each had
/// seen. Replay has taken what each event does: what is left is what `saw` and `who` read.
#[derive(Clone, Debug, Default)]
pub(crate) struct Knowledge {
    pub(crate) events: IndexMap<EventId, Event>,
    /// The machines with a journal, in the journal's order.
    pub(crate) replicas: Vec<String>,
    /// Each machine's positions that hold an applicable event (not a placeholder), ascending.
    pub(crate) applicable: Vec<Vec<u32>>,
    pub(crate) ancestry: Ancestry,
    pub(crate) chain_breaks: IndexMap<String, Vec<usize>>,
}

impl Knowledge {
    /// What `events` had seen, to be observed, given each machine's append `order` and which events
    /// are placeholders, by index.
    pub(crate) fn new(
        events: IndexMap<EventId, Event>,
        order: &IndexMap<String, Vec<EventId>>,
        placeholder: impl Fn(usize) -> bool,
    ) -> Knowledge {
        let replicas: Vec<String> = order.keys().cloned().collect();
        let mut places = vec![Place::Nowhere; events.len()];
        let mut applicable = Vec::with_capacity(replicas.len());
        for (replica, ids) in order.values().enumerate() {
            let mut positions = Vec::new();
            for (number, id) in ids.iter().enumerate() {
                let Some(at) = events.get_index_of(id) else {
                    continue;
                };
                if let Some(place) = places.get_mut(at) {
                    *place = Place::Sequenced(small(replica), small(number));
                }
                if !placeholder(at) {
                    positions.push(small(number));
                }
            }
            applicable.push(positions);
        }
        let ancestry = Ancestry {
            places,
            clocks: vec![UNOBSERVED; events.len()],
            entries: Vec::new(),
            sequenced: small(replicas.len()),
            lengths: Vec::new(),
            last: HashMap::new(),
        };
        Knowledge {
            events,
            replicas,
            applicable,
            ancestry,
            chain_breaks: IndexMap::new(),
        }
    }

    /// Whether the writer of `event` had already seen `other`.
    pub(crate) fn saw(&self, event: EventId, other: EventId) -> bool {
        self.events
            .get_index_of(&event)
            .is_some_and(|at| self.saw_at(at, other))
    }

    /// Whether the writer of the event at `at` had already seen `other`: it names `other` as a
    /// cause, or `other` is within its clock.
    pub(crate) fn saw_at(&self, at: usize, other: EventId) -> bool {
        if self
            .events
            .get_index(at)
            .is_some_and(|(_, event)| event.deps().contains(&other))
        {
            return true;
        }
        self.events
            .get_index_of(&other)
            .is_some_and(|other| self.ancestry.reaches(at, other))
    }

    /// Who wrote `event`, as bead names an actor, if it was replayed.
    pub(crate) fn who(&self, event: EventId) -> Option<String> {
        let event = self.events.get(&event)?;
        Some(actor_label(event.actor(), event.replica()))
    }
}

#[cfg(test)]
mod tests;
