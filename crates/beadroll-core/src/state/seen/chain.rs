//! A machine's journal chain as replay judges it (`replay::observe_one`): each of its events must
//! build on the last applicable event before it there, through the clocks of its causes, or it is
//! set aside as a broken chain.

use std::collections::HashMap;

use crate::id::EventId;
use crate::order::position;

use super::{Knowledge, Place};

impl Knowledge {
    /// Whether an event of `machine`'s journal, after every one here, with `causes` would not build
    /// on the last applicable event of that journal: what replay sets aside as a broken chain. What
    /// it saw of that chain is the furthest any observed cause's clock saw there, a cause replay did
    /// not observe (pending) seeing nothing, a released quarantined id that names a cause not
    /// observed not seeing it; each cause's clock is read for that chain alone, the first reaching
    /// it enough. No where the machine has no journal here, or no applicable event in it.
    pub(crate) fn misses_predecessor(&self, machine: &str, causes: &[EventId]) -> bool {
        let Some(chain) = (self.replicas.iter()).position(|name| name == machine) else {
            return false;
        };
        let Some(&predecessor) = self.applicable.get(chain).and_then(|at| at.last()) else {
            return false;
        };
        let chain = position(chain);
        let ancestry = &self.ancestry;
        !(causes.iter())
            .filter_map(|cause| self.events.get_index_of(cause))
            .filter(|at| ancestry.observed(*at))
            .any(|at| {
                ancestry
                    .reached(at, chain)
                    .is_some_and(|seen| seen >= predecessor)
            })
    }

    /// Whether `tails` names, for each machine whose journal holds an applicable event, the event
    /// this knowledge places at its last applicable position, and nothing for any other: what a
    /// projection read back must hold, a write building on it. Each machine looked up once.
    pub(crate) fn places_last(&self, tails: &HashMap<String, EventId>) -> bool {
        let mut named = 0;
        for (chain, machine) in self.replicas.iter().enumerate() {
            let Some(&last) = self.applicable.get(chain).and_then(|at| at.last()) else {
                continue;
            };
            let at = (tails.get(machine)).and_then(|tail| self.events.get_index_of(tail));
            if at.map(|at| self.ancestry.place(at)) != Some(Place::Sequenced(position(chain), last))
            {
                return false;
            }
            named += 1;
        }
        named == tails.len()
    }

    /// Whether `machine`'s last applicable event broke its journal chain itself: its clock does
    /// not reach its own position, nor does that of any event built on it, and replay sets aside
    /// every later record of the journal as a broken chain too. A placeholder that broke the chain
    /// is no predecessor, and the records after it are judged by the event before it.
    pub(crate) fn last_broke_chain(&self, machine: &str) -> bool {
        let Some(chain) = (self.replicas.iter()).position(|name| name == machine) else {
            return false;
        };
        let Some(&last) = self.applicable.get(chain).and_then(|at| at.last()) else {
            return false;
        };
        (self.chain_breaks.get(machine)).is_some_and(|breaks| breaks.contains(&(last as usize)))
    }

    /// Whether replay observed the event `id` here, by a record this knowledge keeps of it: its
    /// causes came before it, what an event naming it as a cause beside the frontier may be applied
    /// after. Observed is not taken: replay observes an event it then sets aside, a break in its
    /// journal's chain or one id of several contents, and keeps its clock all the same.
    pub(crate) fn observed(&self, id: EventId) -> bool {
        (self.events.get_index_of(&id)).is_some_and(|at| self.ancestry.observed(at))
    }

    /// Whether replay has not observed the event `id` here: it waits for a cause, pending, and so
    /// does every event built on it. Told by its clock, without the pending events; no for an
    /// event this knowledge does not hold.
    pub(crate) fn waits(&self, id: EventId) -> bool {
        (self.events.get_index_of(&id)).is_some_and(|at| !self.ancestry.observed(at))
    }

    /// Each machine's last applicable event in its journal, by id: what its next event must build
    /// on. One pass over the events, each compared with its machine's last applicable position.
    pub(crate) fn predecessors(&self) -> HashMap<String, EventId> {
        let mut found = HashMap::new();
        for (at, id) in self.events.keys().enumerate() {
            let Place::Sequenced(chain, number) = self.ancestry.place(at) else {
                continue;
            };
            let last = (self.applicable.get(chain as usize)).and_then(|at| at.last());
            if last == Some(&number)
                && let Some(machine) = self.replicas.get(chain as usize)
            {
                found.insert(machine.clone(), *id);
            }
        }
        found
    }
}
