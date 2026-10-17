//! Events applied after all of a projection's, when replay would apply them so.
//!
//! Replay applies events in causal order, ties broken by id. A new event that has every event of
//! the frontier in its past comes after every event the journal held: each of those is in some
//! head's past. When every new event does, replay applies the old events as before and the new
//! ones after them, in the order their own causes give, and each new event sees what it saw in a
//! replay: the projection keeps every event the state names, and the frontier, with its place and
//! what it had seen (and between compactions every event taken in since, which it saw as replay
//! did: roll-lqjj.43). A local write's events build on the frontier; so do a merge's, when one
//! writer worked at a time or when the frontier is that of a projection saved before this
//! machine's writes that the merged events are concurrent with (roll-lqjj.36): those writes and
//! the merged events are new together, one batch, and each has the frontier in its past.
//!
//! Anything replay would do otherwise and the projection declines, before it changes anything:
//! a takeover, which can turn an earlier end of its claim stale; a record that is not new, or not
//! its machine's own, or unreadable; a new machine's journal, since chains are numbered by machine;
//! a cause that is neither new nor on the frontier. An event that cannot apply, which replay
//! reports in an order of its own, is found as it is observed: the projection is then dropped.
//!
//! What replay reports does not stop the projection (roll-lqjj.28): a newer bead's operation, a
//! payload or a record that cannot be used, an unreadable line, an id with two contents, a broken
//! chain, a refused takeover. Replay sets each aside from what nothing after can change: the records
//! of its id, its causes and its machine's journal before it, and for a takeover the claim it names;
//! and it lists them in an order nothing after can change either, those found as the journal was
//! read first, then the rest in causal order, where every new event comes after them. A batch that
//! would add one is declined, so the list stays replay's. What a record still to come can change,
//! the projection declines too. A record of an id an integrity entry names settles anew an id found
//! only in another machine's journal (an event, or two contents): such a batch is declined. A
//! takeover refused for naming an id no event has is refused for another reason once an event has
//! it; and an id with several contents waits only for the causes the journal holds, so one it names
//! that is still to come, or causes that loop back onto it (it is then released after every other
//! event), place it after events still to come: a journal holding either is not extended at all
//! ([`Projection::extendable`]), nor is one with an event waiting for a cause, which replay applies
//! once the cause comes.
//!
//! An event that changes an issue's texts (a long field set, a comment, a delete's record, an edge
//! added or removed, whose additions' metadata is with them, roll-lqjj.39), or may (a claim, a
//! release, a close or a reopen, which can add to the ownership history they hold, roll-lqjj.40),
//! needs them in the issue, and a create needs those of each issue that waits for its key, in which
//! it makes an edge: a store reads them in before it extends ([`Projection::about`], roll-lqjj.6),
//! and when one is not, the projection declines too. Texts read in that change are written again.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use indexmap::IndexMap;

use crate::id::EventId;
use crate::journal::{Event, Journal, Parts, Rest};
use crate::json::Value;
use crate::op::Op;
use crate::replay::{Names, Room, apply_one, finish_some, observe_one};
use crate::state::{KeySpace, Texts, is_text};

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
        // an id replay reported a record of, held or not (found only in another machine's journal)
        let reported: HashSet<EventId> = (self.state.integrity.iter())
            .filter_map(|entry| entry.event.as_deref().and_then(EventId::parse))
            .collect();
        if !journal.invalid.is_empty()
            || !journal.quarantined().is_empty()
            || journal.events().len() != count
            || (journal.events().keys()).any(|id| self.ids.contains(id) || reported.contains(id))
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
        if !self.texts_in(&doing) {
            return Ok(Extended::Replay);
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
        finish_some(&mut self.state, &doing);
        self.advance(&events, &order);
        Ok(Extended::Applied(Box::new(self)))
    }

    /// The issues whose texts applying `records` can change, which a store reads in before it
    /// extends ([`Projection::extend`]): each issue a record is about, and each that waits, by an
    /// imported dependency, for the key a create among them brings, which that create makes an edge
    /// of, its metadata with the issue's texts (roll-lqjj.39). Which of them a record needs read in
    /// is [`Projection::texts_in`]'s to say.
    pub fn about(&self, records: &[Record]) -> Vec<EventId> {
        let mut about = Vec::new();
        for record in records {
            let value = &record.value;
            about.extend(
                value
                    .get("issue")
                    .and_then(Value::as_str)
                    .and_then(EventId::parse),
            );
            if value.get("op").and_then(Value::as_str) == Some("create")
                && let Some(key) = value.get("data").and_then(|data| data.get("key"))
                && let Some(key) = key.as_str()
                && !self.state.by_key.contains_key(key)
            {
                about.extend(self.state.awaiting.waiting(key));
            }
        }
        about
    }

    /// Whether every issue whose texts `doing` changes, or may, has them in it, read in by the
    /// store first (roll-lqjj.6, [`Projection::about`]): those read in that it changes are then to
    /// be written again. An edge's metadata is with them (roll-lqjj.39), so an edge added or
    /// removed changes them, and so does a create in each issue that waits for its key. An
    /// ownership event may add to the issue's ownership history, which they hold too
    /// (roll-lqjj.40): whether it does, replay knows once it applies it, and marks them then
    /// (`Issue::ended`, `Issue::lost`, `Issue::ignored`).
    fn texts_in(&mut self, doing: &[Box<Rest>]) -> bool {
        for what in doing {
            // whether it needs the issue's texts, and whether it changes them whatever it does
            let (needs, changes) = match &what.op {
                Op::Set(fields) => {
                    let texts = fields.iter().any(|(field, _)| is_text(*field));
                    (texts, texts)
                }
                Op::Comment { .. } | Op::Delete(_) | Op::DepAdd { .. } | Op::DepRemove { .. } => {
                    (true, true)
                }
                Op::Claim(_) | Op::Release(_) | Op::Close(_) | Op::Reopen => (true, false),
                Op::Create(making) if !self.state.by_key.contains_key(&making.key) => {
                    for uid in self.state.awaiting.waiting(&making.key) {
                        if !self.texts_here(uid, true) {
                            return false;
                        }
                    }
                    (false, false)
                }
                _ => (false, false),
            };
            let issue = (what.issue.as_deref()).and_then(EventId::parse);
            if let Some(uid) = issue.filter(|_| needs)
                && !self.texts_here(uid, changes)
            {
                return false;
            }
        }
        true
    }

    /// Whether issue `uid`'s texts are in it, if it is an issue, to be written again when `changes`:
    /// `false` when they are only in the text file. Texts read in that change are then to be placed
    /// anew, which marks the issue; otherwise it is left as it is.
    fn texts_here(&mut self, uid: EventId, changes: bool) -> bool {
        match self.state.issues.get(&uid).map(|issue| &issue.texts) {
            None | Some(Texts::Here) => true,
            Some(Texts::There(_)) => false,
            Some(Texts::Read(_)) => {
                if changes && let Some(issue) = self.state.issues.get_mut(&uid) {
                    issue.texts = Texts::Here;
                }
                true
            }
        }
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

    /// The frontier, the ids and the journals' lengths and latest events with the new `events` and
    /// their machines' records (`order`), once they apply. Whether more can be applied after stays
    /// as it was: a batch that waits for a cause or holds a takeover is declined.
    fn advance(
        &mut self,
        events: &IndexMap<EventId, Event>,
        order: &IndexMap<String, Vec<EventId>>,
    ) {
        let named: HashSet<EventId> = (events.values())
            .flat_map(|event| event.deps().iter().copied())
            .collect();
        self.heads = (events.keys())
            .filter(|id| !named.contains(id))
            .copied()
            .collect();
        self.heads.sort_unstable();
        self.ids.extend(events.keys().copied());
        for (machine, added) in order {
            if let Some(length) = self.lengths.get_mut(machine) {
                *length = length.saturating_add(small(added.len()));
            }
            if let Some(&latest) = added.iter().max() {
                let last = self.lasts.entry(machine.clone()).or_insert(latest);
                *last = (*last).max(latest);
            }
        }
    }
}
