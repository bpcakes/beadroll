//! A borrowed proof that all records have been judged. Only the journal constructs it.

use indexmap::IndexMap;

use super::{Event, Journal, Problem};
use crate::id::EventId;

/// A journal's settled collections, held unchanged for this borrow.
#[derive(Clone, Copy, Debug)]
pub struct SettledJournal<'a>(&'a Journal);

impl<'a> SettledJournal<'a> {
    pub(crate) fn new(journal: &'a Journal) -> Self {
        Self(journal)
    }

    /// The events, in installation order.
    pub fn events(self) -> &'a IndexMap<EventId, Event> {
        self.0.events()
    }

    /// The event with this id, if it has a place in the journal.
    pub fn event(self, id: EventId) -> Option<&'a Event> {
        self.0.event(id)
    }

    /// Each machine's append order.
    pub fn order(self) -> &'a IndexMap<String, Vec<EventId>> {
        self.0.order()
    }

    /// Every id found with several contents.
    pub fn quarantined(self) -> &'a IndexMap<EventId, Vec<Event>> {
        self.0.quarantined()
    }

    /// Records and files that could not be used.
    pub fn invalid(self) -> &'a [Problem] {
        &self.0.invalid
    }

    /// Events nothing here builds on, sorted by id.
    pub fn frontier(self) -> Vec<EventId> {
        self.0.frontier()
    }
}
