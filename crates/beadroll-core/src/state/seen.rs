//! What each event's writer had seen (Python's `State.seq`, `seen`, `own_anc` and `saw`): for every
//! machine's journal, the furthest position in its past, and which unsequenced events (an attempt
//! file's, which have no position) are its ancestors.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::id::EventId;
use crate::journal::Journal;

/// A set of small numbers: the unsequenced ancestors of an event, by the bit each was given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Bits(Vec<u64>);

impl Bits {
    pub(crate) fn insert(&mut self, bit: usize) {
        let (word, offset) = (bit / 64, bit % 64);
        if self.0.len() <= word {
            self.0.resize(word + 1, 0);
        }
        if let Some(slot) = self.0.get_mut(word) {
            *slot |= 1 << offset;
        }
    }

    pub(crate) fn union(&mut self, other: &Bits) {
        if self.0.len() < other.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        for (mine, theirs) in self.0.iter_mut().zip(&other.0) {
            *mine |= theirs;
        }
    }

    pub(crate) fn contains(&self, bit: usize) -> bool {
        self.0
            .get(bit / 64)
            .is_some_and(|word| word >> (bit % 64) & 1 == 1)
    }
}

/// The furthest position seen in each machine's journal, by the machine's index: sorted, sparse.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Seen(Vec<(usize, i64)>);

impl Seen {
    pub(crate) fn get(&self, replica: usize) -> i64 {
        self.0
            .binary_search_by_key(&replica, |(r, _)| *r)
            .ok()
            .and_then(|at| self.0.get(at))
            .map_or(-1, |(_, number)| *number)
    }

    /// Takes the furthest of both, machine by machine.
    pub(crate) fn merge(&mut self, other: &Seen) {
        let mut merged = Vec::with_capacity(self.0.len().max(other.0.len()));
        let (mut mine, mut theirs) = (
            self.0.iter().copied().peekable(),
            other.0.iter().copied().peekable(),
        );
        loop {
            let next = match (mine.peek().copied(), theirs.peek().copied()) {
                (Some(a), Some(b)) if a.0 == b.0 => {
                    mine.next();
                    theirs.next();
                    (a.0, a.1.max(b.1))
                }
                (Some(a), Some(b)) if a.0 < b.0 => {
                    mine.next();
                    a
                }
                (_, Some(b)) => {
                    theirs.next();
                    b
                }
                (Some(a), None) => {
                    mine.next();
                    a
                }
                (None, None) => break,
            };
            merged.push(next);
        }
        self.0 = merged;
    }

    pub(crate) fn raise(&mut self, replica: usize, number: i64) {
        match self.0.binary_search_by_key(&replica, |(r, _)| *r) {
            Ok(at) => {
                if let Some(slot) = self.0.get_mut(at) {
                    slot.1 = slot.1.max(number);
                }
            }
            Err(at) => self.0.insert(at, (replica, number)),
        }
    }
}

/// The journal, and what each of its events had seen.
#[derive(Clone, Debug, Default)]
pub(crate) struct Knowledge {
    pub(crate) journal: Journal,
    /// The machines with a journal, in the journal's order.
    pub(crate) replicas: Vec<String>,
    /// Each sequenced event's machine (by index) and position in that machine's journal.
    pub(crate) seq: HashMap<EventId, (usize, usize)>,
    pub(crate) seen: HashMap<EventId, Seen>,
    pub(crate) own_anc: HashMap<EventId, Bits>,
    /// The bit each unsequenced ancestor was given, in the order first met.
    pub(crate) bit: HashMap<EventId, usize>,
    pub(crate) chain_breaks: IndexMap<String, Vec<usize>>,
}

impl Knowledge {
    pub(crate) fn new(journal: Journal) -> Knowledge {
        let replicas: Vec<String> = journal.order().keys().cloned().collect();
        let mut seq = HashMap::new();
        for (index, ids) in journal.order().values().enumerate() {
            for (number, id) in ids.iter().enumerate() {
                seq.insert(*id, (index, number));
            }
        }
        Knowledge {
            journal,
            replicas,
            seq,
            seen: HashMap::new(),
            own_anc: HashMap::new(),
            bit: HashMap::new(),
            chain_breaks: IndexMap::new(),
        }
    }

    /// Whether the writer of `event` had already seen `other`.
    pub(crate) fn saw(&self, event: EventId, other: EventId) -> bool {
        if self
            .journal
            .event(event)
            .is_some_and(|e| e.deps().contains(&other))
        {
            return true;
        }
        if let Some((replica, number)) = self.seq.get(&other) {
            let seen = self.seen.get(&event).map_or(-1, |seen| seen.get(*replica));
            return seen >= i64::try_from(*number).unwrap_or(i64::MAX);
        }
        // An unsequenced event has no position: it is an ancestor or not.
        self.bit.get(&other).is_some_and(|bit| {
            self.own_anc
                .get(&event)
                .is_some_and(|anc| anc.contains(*bit))
        })
    }
}
