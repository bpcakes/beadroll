//! What a projection keeps of what every event had seen (roll-lqjj.2): the events its state still
//! names, each with its place and its clock, and enough of every chain to place the events that
//! come after all of them.

use std::collections::{HashMap, HashSet};

use indexmap::{IndexMap, IndexSet};

use crate::id::EventId;
use crate::journal::Event;

use super::{Ancestry, Clock, Knowledge, Place, UNOBSERVED, small};

impl Knowledge {
    /// This knowledge cut down to the events in `keep`, in the journal's order, each with its place
    /// and what it had seen: what applying an event after all of them asks of it. Each machine's
    /// applicable positions are cut to its last, which is all that an event after it compares with.
    pub(crate) fn kept(&self, keep: &HashSet<EventId>) -> Knowledge {
        let at: Vec<usize> = (self.events.keys().enumerate())
            .filter(|(_, id)| keep.contains(id))
            .map(|(at, _)| at)
            .collect();
        let events: IndexMap<EventId, Event> = (at.iter())
            .filter_map(|at| self.events.get_index(*at))
            .map(|(id, event)| (*id, event.clone()))
            .collect();
        let applicable = (self.applicable.iter())
            .map(|positions| positions.last().copied().into_iter().collect())
            .collect();
        Knowledge {
            events,
            replicas: self.replicas.clone(),
            applicable,
            ancestry: self.ancestry.kept(&at),
            chain_breaks: self.chain_breaks.clone(),
        }
    }

    /// Takes `event` after every event here, at position `number` of its machine's journal when it
    /// is one of its records (`None` for an attempt's), and says where it is. Its machine must have
    /// a journal here already when it is: chains are numbered by machine. `None` when that is not
    /// so.
    pub(crate) fn push(&mut self, event: Event, number: Option<u32>) -> Option<usize> {
        let place = match number {
            Some(number) => {
                let chain = (self.replicas.iter()).position(|name| name == event.replica())?;
                let positions = self.applicable.get_mut(chain)?;
                positions.push(number);
                Place::Sequenced(small(chain), number)
            }
            None => Place::Nowhere,
        };
        let at = self.events.len();
        self.events.insert(event.id(), event);
        self.ancestry.places.push(place);
        self.ancestry.clocks.push(UNOBSERVED);
        Some(at)
    }
}

impl Ancestry {
    /// The places and clocks of the events at `at` alone, renumbered in that order; the chains as
    /// they are.
    fn kept(&self, at: &[usize]) -> Ancestry {
        let mut out = Ancestry {
            places: Vec::with_capacity(at.len()),
            clocks: Vec::with_capacity(at.len()),
            sets: IndexSet::new(),
            positions: Vec::new(),
            sequenced: self.sequenced,
            lengths: self.lengths.clone(),
            last: self.last.clone(),
        };
        // A clock several events share stays shared.
        let mut moved: HashMap<Clock, Clock> = HashMap::new();
        for &at in at {
            out.places.push(self.place(at));
            let clock = self.clocks.get(at).copied().unwrap_or(UNOBSERVED);
            if clock == UNOBSERVED {
                out.clocks.push(UNOBSERVED);
                continue;
            }
            let kept = *moved.entry(clock).or_insert_with(|| {
                let seen = self.read(clock);
                let set = out.sets.insert_full(seen.chains.into()).0;
                let start = out.positions.len();
                out.positions.extend_from_slice(seen.positions);
                Clock {
                    set: small(set),
                    start,
                }
            });
            out.clocks.push(kept);
        }
        out
    }
}
