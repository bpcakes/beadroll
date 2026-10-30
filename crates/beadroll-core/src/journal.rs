//! Reads and settles records from all journal files together, so duplicate and malformed records
//! are judged independently of file-read order. Keeps only what replay needs from each record
//! ([`Event`], roll-dih8.11): a line is read straight into that, with no value built for it
//! ([`Journal::add_segment`], [`Journal::add_attempt`], roll-lqjj.53.1), and a record built in
//! memory is read as a line of it would be ([`Journal::add`]).

mod content;
mod event;
mod read;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::Arc;

use content::Digest;
use event::{Shared, Source};
use indexmap::IndexMap;

use crate::event::Envelope;
use crate::id::EventId;
use crate::json::{self, Format, Value};
use crate::op::Op;

/// Whether a record belongs to the machine journal order or an ownership attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalOrder {
    /// Attempt.
    Attempt,
    /// Sequenced.
    Sequenced,
}

pub use event::Event;
pub(crate) use event::Rest;
pub use read::{loads_line, read_attempt, read_segment};

/// Something a reader could not use: an unreadable line or file, an event that fails validation, a
/// copy in the wrong journal. Replay reports each as an integrity row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The event's id, when one could be read.
    pub event: Option<String>,
    /// Where it was found: a path, with `:LINE` for a line.
    pub origin: String,
    /// The diagnostic describing the problem.
    pub problem: String,
}

impl Problem {
    fn new(event: Option<String>, origin: String, problem: impl Into<String>) -> Problem {
        Problem {
            event,
            origin,
            problem: problem.into(),
        }
    }
}

/// All events held by a machine, retaining each writer's append order. Raw records are collected
/// before settling.
#[derive(Clone, Debug)]
pub struct Journal {
    events: IndexMap<EventId, Event>,
    order: IndexMap<String, Vec<EventId>>,
    quarantined: IndexMap<EventId, Vec<Event>>,
    /// What could not be used, in the order found; replay reports each.
    pub invalid: Vec<Problem>,
    /// Every record with a readable id, in the order added.
    raw: Vec<Event>,
    /// Ids that have records but no event: every record of them was found in another machine's
    /// journal. Every other id with a record has its event, once settled.
    unplaced: HashSet<EventId>,
    counter: u64,
    settled: bool,
    /// The texts and values events repeat, held once.
    shared: Shared,
    /// Where the last record was found: the next ones from its file share it.
    last_source: Option<Arc<Source>>,
}

/// A settled journal taken apart for replay (roll-dih8.11): its events by id, in the order
/// installed, each machine's append order, the ids found with several contents, and what could not
/// be used. Its raw records go with the rest: replay reads each event once.
pub(crate) struct Parts {
    pub(crate) events: IndexMap<EventId, Event>,
    pub(crate) order: IndexMap<String, Vec<EventId>>,
    pub(crate) quarantined: HashSet<EventId>,
    pub(crate) invalid: Vec<Problem>,
}

impl Default for Journal {
    fn default() -> Journal {
        Journal::new()
    }
}

impl Journal {
    /// An empty journal.
    pub fn new() -> Journal {
        Journal {
            events: IndexMap::new(),
            order: IndexMap::new(),
            quarantined: IndexMap::new(),
            invalid: Vec::new(),
            raw: Vec::new(),
            unplaced: HashSet::new(),
            counter: 0,
            settled: true,
            shared: Shared::default(),
            last_source: None,
        }
    }

    /// Adds a record built in memory, found in `replica`'s journal at `origin`;
    /// [`JournalOrder::Attempt`] events have no place in the journal's order. It is read
    /// as the line bead writes of it would be. Anything without a readable id is reported at once;
    /// the rest is judged when the journal settles.
    pub fn add(&mut self, event: Value, replica: &str, origin: &str, journal_order: JournalOrder) {
        let digest = self.shared.digester.digest_of(&event);
        self.add_read(
            Envelope::from_value(event),
            digest,
            replica,
            origin,
            journal_order,
        );
    }

    /// Adds what was read of a record found in `replica`'s journal at `origin`, its content's
    /// digest being `digest`.
    fn add_read(
        &mut self,
        envelope: Envelope,
        digest: Digest,
        replica: &str,
        origin: &str,
        journal_order: JournalOrder,
    ) {
        let Some(id) = envelope.id else {
            let problem = envelope
                .problem()
                .unwrap_or_else(|| "id is not a UUID".to_owned());
            self.invalid
                .push(Problem::new(None, origin.to_owned(), problem));
            return;
        };
        self.counter += 1;
        let source = self.source(replica, origin, journal_order);
        let record = Event::read(
            id,
            envelope,
            digest,
            &source,
            self.counter,
            &mut self.shared,
        );
        // The first record of an id is the common case: a fresh, valid event, installed at once.
        let first = !self.events.contains_key(&id) && !self.unplaced.contains(&id);
        let fresh = first && record.own();
        self.raw.push(record.clone());
        if self.settled && fresh {
            self.install(record);
        } else {
            self.settled = false;
        }
    }

    /// Where a record of `replica`'s journal at `origin` was found, shared with the records before
    /// it from the same file.
    fn source(&mut self, replica: &str, origin: &str, sequenced: JournalOrder) -> Arc<Source> {
        let sequenced = matches!(sequenced, JournalOrder::Sequenced);
        if let Some(last) = &self.last_source
            && *last.replica == *replica
            && *last.origin == *origin
            && last.sequenced == sequenced
        {
            return Arc::clone(last);
        }
        let source = Arc::new(Source {
            replica: self.shared.text(replica),
            origin: self.shared.text(origin),
            sequenced,
        });
        self.last_source = Some(Arc::clone(&source));
        source
    }

    fn install(&mut self, event: Event) {
        let id = event.id();
        let source = Arc::clone(event.source());
        if source.sequenced {
            match self.order.get_mut(&*source.replica) {
                Some(ids) => ids.push(id),
                None => {
                    self.order.insert(source.replica.to_string(), vec![id]);
                }
            }
        }
        self.events.insert(id, event);
    }

    /// Rebuilds from the raw records: one content per id is an event; several contents are a
    /// quarantined id; a malformed record keeps its place as an inert placeholder; a copy found only
    /// in another machine's journal is ignored.
    pub fn settle(&mut self) {
        if self.settled {
            return;
        }
        self.events.clear();
        self.order.clear();
        self.quarantined.clear();
        self.unplaced.clear();
        self.invalid.retain(|entry| entry.event.is_none()); // unreadable lines and files stay reported
        let raw = std::mem::take(&mut self.raw);
        // Each id's records, ids in the order first found, records in the order added.
        let mut by_id: IndexMap<EventId, Vec<&Event>> = IndexMap::new();
        for record in &raw {
            by_id.entry(record.id()).or_default().push(record);
        }
        let mut placed: Vec<(&Vec<&Event>, bool, &Event)> = Vec::new();
        for records in by_id.values() {
            let own = records.iter().find(|record| record.own()).copied();
            let placement = own.or_else(|| records.first().copied());
            if let Some(placement) = placement {
                placed.push((records, own.is_some(), placement));
            }
        }
        placed.sort_by_key(|(_, _, placement)| placement.counter()); // each journal keeps its own order
        for (records, own, placement) in placed {
            let id = placement.id();
            // Keep each distinct content once, in discovery order.
            let mut distinct: Vec<&Event> = Vec::new();
            for record in records {
                if !distinct.iter().any(|seen| seen.digest() == record.digest()) {
                    distinct.push(record);
                }
            }
            let origin = &placement.source().origin;
            if distinct.len() > 1 {
                self.quarantined
                    .insert(id, distinct.into_iter().cloned().collect());
                let trusted = if own {
                    placement.deps().into()
                } else {
                    Box::default()
                };
                // A copy that is not valid may name anything: only a string is an issue (U10).
                let issue = placement.issue().map(|issue| self.shared.text(issue));
                let stand_in = Event::stand_in(placement, Op::Quarantined, trusted, issue);
                self.install(stand_in);
                continue;
            }
            if let Some(problem) = placement.problem() {
                self.invalid.push(Problem::new(
                    Some(id.to_string()),
                    origin.to_string(),
                    problem,
                ));
                let stand_in =
                    Event::stand_in(placement, Op::Invalid, placement.deps().into(), None);
                self.install(stand_in);
                continue;
            }
            if !own {
                let writer = placement.replica();
                let replica = &placement.source().replica;
                let problem = format!(
                    "written by machine {writer} but found only in machine {replica}'s journal"
                );
                self.invalid.push(Problem::new(
                    Some(id.to_string()),
                    origin.to_string(),
                    problem,
                ));
                self.unplaced.insert(id);
                continue;
            }
            self.install(placement.clone());
        }
        drop(by_id);
        self.raw = raw;
        self.settled = true;
    }

    /// The journal taken apart for replay, settled first.
    pub(crate) fn into_parts(mut self) -> Parts {
        self.settle();
        Parts {
            events: self.events,
            order: self.order,
            quarantined: self.quarantined.into_keys().collect(),
            invalid: self.invalid,
        }
    }

    /// The frontier: events nothing else here has built on yet, in order.
    pub fn heads(&mut self) -> Vec<EventId> {
        self.settle();
        self.frontier()
    }

    /// The frontier of a settled journal (what [`Journal::heads`] answers once it has settled).
    pub fn frontier(&self) -> Vec<EventId> {
        // The events another one builds on, by index: worked out when asked, since a write asks
        // once, where a set kept as events arrive would hold every id again.
        let mut referenced = vec![false; self.events.len()];
        for event in self.events.values() {
            for at in event
                .deps()
                .iter()
                .filter_map(|dep| self.events.get_index_of(dep))
            {
                if let Some(flag) = referenced.get_mut(at) {
                    *flag = true;
                }
            }
        }
        let mut heads: Vec<EventId> = (self.events.keys().zip(referenced))
            .filter(|(_, referenced)| !referenced)
            .map(|(id, _)| *id)
            .collect();
        heads.sort_unstable();
        heads
    }

    /// The events, in the order they were installed. Settle first.
    pub fn events(&self) -> &IndexMap<EventId, Event> {
        &self.events
    }

    /// The event `id`, if this journal holds it.
    pub fn event(&self, id: EventId) -> Option<&Event> {
        self.events.get(&id)
    }

    /// Each machine's events, in its append order.
    pub fn order(&self) -> &IndexMap<String, Vec<EventId>> {
        &self.order
    }

    /// Ids found with several contents: a record of each content, in the order found.
    pub fn quarantined(&self) -> &IndexMap<EventId, Vec<Event>> {
        &self.quarantined
    }

    /// Whether every record added has been judged.
    pub fn is_settled(&self) -> bool {
        self.settled
    }
}

/// Compact JSON with sorted keys, for tests and tools comparing values independently of member
/// order. Quarantine compares line-byte digests instead.
pub fn canonical(event: &Value) -> String {
    // Only a value built in memory fails to print (an integer too long, nesting too deep); its
    // debug form still tells contents apart.
    json::write(event, Format::CANONICAL).unwrap_or_else(|_| format!("{event:?}"))
}
