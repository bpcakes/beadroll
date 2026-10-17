//! Ancestry represented by one clock per event over chains. Each machine's journal is a chain;
//! unsequenced attempt events join a chain whose last event is already an ancestor, preferring
//! their machine's chain, or start a new chain. Every ancestor set within a chain is a prefix, so
//! one position represents it. Writers build on all events they hold, keeping chain counts near
//! the number of active writers.
//!
//! Each event's clock merges its causes' furthest positions by chain, adds unsequenced causes and
//! records its own journal position when sequenced. Clocks share an arena, and an unchanged clock
//! reuses its cause's storage. Chain sets are interned separately, leaving only positions in the
//! arena. This avoids per-event bitsets growing with all ownership events.
//!
//! A journal contains fewer than 2³² events, each costing hundreds of bytes in memory, so chain
//! ids and positions use u32.

use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};

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

/// A clock as recorded: the number of the set of chains it has seen into, and where its positions
/// in them begin in the arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Clock {
    set: u32,
    start: usize,
}

/// The clock of an event not observed: it has seen nothing.
const UNOBSERVED: Clock = Clock {
    set: u32::MAX,
    start: usize::MAX,
};

/// The least the arena grows by, in positions.
const GROWTH: usize = 1 << 12;

/// A recorded clock read back: the chains it has seen into, ascending, and the furthest position
/// it saw in each.
#[derive(Clone, Copy)]
struct Seen<'a> {
    chains: &'a [u32],
    positions: &'a [u32],
}

impl Seen<'_> {
    /// The furthest position this clock saw in `chain`.
    fn reached(self, chain: u32) -> Option<u32> {
        let at = self.chains.binary_search(&chain).ok()?;
        self.positions.get(at).copied()
    }

    /// Its entries: (chain, furthest position seen), by chain.
    fn entries(self) -> impl Iterator<Item = (u32, u32)> {
        self.chains
            .iter()
            .copied()
            .zip(self.positions.iter().copied())
    }
}

/// What every event had seen, by event index.
#[derive(Clone, Debug, Default)]
pub(crate) struct Ancestry {
    /// Each event's place.
    places: Vec<Place>,
    /// Each event's clock.
    clocks: Vec<Clock>,
    /// The sets of chains the clocks have seen into, each ascending.
    sets: IndexSet<Box<[u32]>>,
    /// The clocks' positions: for each chain of a clock's set, the furthest position it saw there.
    positions: Vec<u32>,
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

    /// The clock of the event at `at`: it has seen nothing when it was not observed.
    fn clock(&self, at: usize) -> Seen<'_> {
        self.read(self.clocks.get(at).copied().unwrap_or(UNOBSERVED))
    }

    /// A recorded clock, read back.
    fn read(&self, clock: Clock) -> Seen<'_> {
        let chains = (self.sets.get_index(clock.set as usize)).map_or(&[][..], |set| &set[..]);
        let end = clock.start.saturating_add(chains.len());
        match self.positions.get(clock.start..end) {
            Some(positions) => Seen { chains, positions },
            None => Seen {
                chains: &[],
                positions: &[],
            },
        }
    }

    /// Whether the event at `at` has been observed.
    pub(crate) fn observed(&self, at: usize) -> bool {
        self.clocks
            .get(at)
            .is_some_and(|clock| *clock != UNOBSERVED)
    }

    /// The furthest position the event at `at` saw in `chain`.
    fn reached(&self, at: usize, chain: u32) -> Option<u32> {
        self.clock(at).reached(chain)
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
        causes: &[u32],
        out: &mut Vec<(u32, u32)>,
        spare: &mut Vec<(u32, u32)>,
    ) {
        out.clear();
        // A released quarantined id may name a cause that never applied.
        let mut observed = (causes.iter())
            .map(|cause| *cause as usize)
            .filter(|cause| self.observed(*cause));
        let Some(first) = observed.next() else {
            return;
        };
        out.extend(self.clock(first).entries());
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
        causes: &[u32],
        replica: &str,
    ) {
        let same = self
            .recorded(causes)
            .find(|recorded| self.read(*recorded).entries().eq(clock.iter().copied()));
        let recorded = match same {
            Some(recorded) => recorded,
            None => Clock {
                set: self.set_of(clock, causes),
                start: self.append(clock),
            },
        };
        if let Some(slot) = self.clocks.get_mut(at) {
            *slot = recorded;
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

    /// The recorded clocks of the observed events among `causes`.
    fn recorded(&self, causes: &[u32]) -> impl Iterator<Item = Clock> {
        (causes.iter())
            .filter_map(|cause| self.clocks.get(*cause as usize).copied())
            .filter(|clock| *clock != UNOBSERVED)
    }

    /// The number of the set of chains `clock` has seen into, which is new when no clock has seen
    /// into just those. Most clocks have seen into the chains one of their `causes` saw into.
    fn set_of(&mut self, clock: &[(u32, u32)], causes: &[u32]) -> u32 {
        let chains = clock.iter().map(|(chain, _)| *chain);
        let known = self.recorded(causes).map(|cause| cause.set).find(|set| {
            (self.sets.get_index(*set as usize))
                .is_some_and(|set| set.iter().copied().eq(chains.clone()))
        });
        known.unwrap_or_else(|| small(self.sets.insert_full(chains.collect()).0))
    }

    /// Adds `clock`'s positions to the arena, and says where they begin.
    fn append(&mut self, clock: &[(u32, u32)]) -> usize {
        let start = self.positions.len();
        if self.positions.capacity() - start < clock.len() {
            // The arena is the largest thing replay builds: grown by an eighth at a time, it holds
            // at most an eighth more than it needs, where doubling it could hold twice as much.
            let more = clock.len().max(start / 8).max(GROWTH);
            self.positions.reserve_exact(more);
        }
        self.positions
            .extend(clock.iter().map(|(_, position)| *position));
        start
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
fn merge(clock: &mut Vec<(u32, u32)>, other: Seen<'_>, spare: &mut Vec<(u32, u32)>) {
    spare.clear();
    let (mut mine, mut theirs) = (clock.iter().copied().peekable(), other.entries().peekable());
    loop {
        let next = match (mine.peek(), theirs.peek()) {
            (Some(a), Some(b)) if a.0 == b.0 => {
                let entry = (a.0, a.1.max(b.1));
                mine.next();
                theirs.next();
                entry
            }
            (Some(a), Some(b)) if a.0 < b.0 => {
                let entry = *a;
                mine.next();
                entry
            }
            (_, Some(b)) => {
                let entry = *b;
                theirs.next();
                entry
            }
            (Some(a), None) => {
                let entry = *a;
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
            sets: IndexSet::new(),
            positions: Vec::new(),
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

    /// Everything kept of the event at `at`, as text: its id, causes, machine and actor, its place,
    /// and what it had seen (the chains and the furthest position in each). What a write-back may
    /// copy of it as it was read (roll-lqjj.44) must be this, unchanged.
    #[cfg(test)]
    pub(crate) fn record_of(&self, at: usize) -> Option<String> {
        let (id, event) = self.events.get_index(at)?;
        let seen: Vec<(u32, u32)> = self.ancestry.clock(at).entries().collect();
        Some(format!(
            "{id} {:?} {} {:?} {:?} {seen:?}",
            event.deps(),
            event.replica(),
            event.actor(),
            self.ancestry.place(at),
        ))
    }
}

mod codec;
mod compact;

pub(crate) use codec::Arena;

#[cfg(test)]
mod tests;
