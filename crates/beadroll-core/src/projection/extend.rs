//! Events applied after all of a projection's, when replay would apply them so.
//!
//! Replay applies events in causal order, ties broken by id. A new event that has every event of
//! the frontier in its past comes after every event the journal held: each of those is in some
//! head's past. When every new event does, replay applies the old events as before and the new
//! ones after them, in the order their own causes give, and each new event sees what it saw in a
//! replay: the projection keeps every event the state names, and the frontier, with its place and
//! what it had seen. A local write's events build on the frontier; so do a merge's, when one
//! writer worked at a time.
//!
//! Anything replay would do otherwise and the projection declines, before it changes anything:
//! a takeover, which can turn an earlier end of its claim stale; a record that is not new, or not
//! its machine's own, or unreadable; a new machine's journal, since chains are numbered by machine;
//! a cause that is neither new nor on the frontier. An event that cannot apply, which replay
//! reports in an order of its own, is found as it is observed: the projection is then dropped.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use indexmap::IndexMap;

use crate::id::EventId;
use crate::journal::{Event, Journal, Parts};
use crate::op::Op;
use crate::replay::{Names, Room, apply_one, finish, observe_one};
use crate::state::KeySpace;

use super::{Projection, Record, small};

/// What became of a projection that events were applied to.
#[derive(Debug)]
pub enum Extended {
    /// Applied as replay would apply them: the projection, with them.
    Applied(Box<Projection>),
    /// Replay would apply them otherwise: replay the journal.
    Replay,
}

impl Projection {
    /// Applies `records`, read from the journal's files after the records this projection was made
    /// from, as replay would apply them after every event of that journal; or declines
    /// ([`Extended::Replay`]). Fails only as replay does, when a renamed copy finds no free key.
    pub fn extend(mut self, records: Vec<Record>) -> Result<Extended, KeySpace> {
        if records.is_empty() {
            return Ok(Extended::Applied(Box::new(self)));
        }
        if !self.open {
            return Ok(Extended::Replay);
        }
        // Read and judged as a journal reads and judges them: every record a new, valid event of
        // the machine whose folder held it.
        let count = records.len();
        let mut journal = Journal::new();
        for record in records {
            let Record {
                value,
                replica,
                origin,
                sequenced,
            } = record;
            journal.add(value, &replica, &origin, sequenced);
        }
        journal.settle();
        if !journal.invalid.is_empty()
            || !journal.quarantined().is_empty()
            || journal.events().len() != count
            || (journal.events().keys()).any(|id| self.ids.binary_search(id).is_ok())
        {
            return Ok(Extended::Replay);
        }
        let Parts {
            mut events, order, ..
        } = journal.into_parts();
        // Each new record of a machine's journal at its place there, after the ones held.
        let mut numbers: HashMap<EventId, u32> = HashMap::new();
        for (machine, added) in &order {
            let Some(length) = self.lengths.get(machine).copied() else {
                return Ok(Extended::Replay);
            };
            for (n, id) in added.iter().enumerate() {
                numbers.insert(*id, length.saturating_add(small(n)));
            }
        }
        let mut doing = Vec::with_capacity(count);
        for event in events.values_mut() {
            let what = event.take_rest();
            if let Op::Claim(claiming) = &what.op
                && claiming.takeover.as_deref().is_some_and(|t| !t.is_empty())
            {
                return Ok(Extended::Replay);
            }
            doing.push(what);
        }
        let Some(ordered) = self.after_all(&events) else {
            return Ok(Extended::Replay);
        };
        let mut knowledge = std::mem::take(&mut self.state.knowledge);
        // Every new record in its place first, as replay places every record before it observes
        // any: whether one builds on its predecessor is asked of the predecessor in the journal,
        // which may come later in the order they apply in.
        let mut placed = Vec::with_capacity(events.len());
        for (id, event) in &events {
            let Some(at) = knowledge.push(event.clone(), numbers.get(id).copied()) else {
                return Ok(Extended::Replay);
            };
            placed.push(at);
        }
        let (mut room, names) = (Room::default(), Names::default());
        for at in ordered {
            let (Some((_, event)), Some(what), Some(&placed)) =
                (events.get_index(at), doing.get(at), placed.get(at))
            else {
                return Ok(Extended::Replay);
            };
            let causes: Vec<u32> = (event.deps().iter())
                .filter_map(|dep| knowledge.events.get_index_of(dep))
                .map(small)
                .collect();
            if observe_one(&mut knowledge, placed, &causes, what, false, &mut room).is_some() {
                return Ok(Extended::Replay);
            }
            apply_one(
                &mut self.state,
                &knowledge,
                placed,
                what,
                None,
                &self.superseded,
                &names,
            )?;
        }
        self.state.knowledge = knowledge;
        finish(&mut self.state);
        self.advance(&events, &order);
        Ok(Extended::Applied(Box::new(self)))
    }

    /// The new `events` in the order replay applies them after the projection's, when every one
    /// has the whole frontier in its past: their causal order, ties broken by id. `None` when one
    /// does not, or names a cause that is neither on the frontier nor new, or never comes.
    fn after_all(&self, events: &IndexMap<EventId, Event>) -> Option<Vec<usize>> {
        let words = self.heads.len().div_ceil(64);
        // Which heads each new event has in its past, a bit each.
        let mut past = vec![vec![0u64; words]; events.len()];
        let mut waiting = vec![0usize; events.len()];
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); events.len()];
        let mut ready = BinaryHeap::new();
        for (at, (id, event)) in events.iter().enumerate() {
            for dep in event.deps() {
                if let Ok(head) = self.heads.binary_search(dep) {
                    let word = past.get_mut(at)?.get_mut(head / 64)?;
                    *word |= 1 << (head % 64);
                } else {
                    let cause = events.get_index_of(dep)?;
                    *waiting.get_mut(at)? += 1;
                    dependents.get_mut(cause)?.push(at);
                }
            }
            if waiting.get(at) == Some(&0) {
                ready.push(Reverse((*id, at)));
            }
        }
        let every = |seen: &[u64]| {
            (0..self.heads.len()).all(|head| {
                (seen.get(head / 64)).is_some_and(|word| word & (1 << (head % 64)) != 0)
            })
        };
        let mut ordered = Vec::with_capacity(events.len());
        while let Some(Reverse((_, at))) = ready.pop() {
            let seen = past.get(at)?.clone();
            if !every(&seen) {
                return None;
            }
            ordered.push(at);
            for &child in dependents.get(at)? {
                for (word, bits) in past.get_mut(child)?.iter_mut().zip(&seen) {
                    *word |= bits;
                }
                let left = waiting.get_mut(child)?;
                *left -= 1;
                if *left == 0 {
                    ready.push(Reverse((*events.get_index(child)?.0, child)));
                }
            }
        }
        (ordered.len() == events.len()).then_some(ordered)
    }

    /// The frontier, the ids and the journals' lengths with the new `events` and their machines'
    /// records (`order`), once they apply.
    fn advance(
        &mut self,
        events: &IndexMap<EventId, Event>,
        order: &IndexMap<String, Vec<EventId>>,
    ) {
        let named: std::collections::HashSet<EventId> = (events.values())
            .flat_map(|event| event.deps().iter().copied())
            .collect();
        self.heads = (events.keys())
            .filter(|id| !named.contains(id))
            .copied()
            .collect();
        self.heads.sort_unstable();
        self.ids.extend(events.keys().copied());
        self.ids.sort_unstable();
        for (machine, added) in order {
            if let Some(length) = self.lengths.get_mut(machine) {
                *length = length.saturating_add(small(added.len()));
            }
        }
        self.open = self.state.integrity.is_empty() && self.state.pending.is_empty();
    }
}
