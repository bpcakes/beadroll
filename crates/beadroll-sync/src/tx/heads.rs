//! What a transaction's next event builds on, by the route it takes. Its records of the journal
//! are appended before its attempt is published, which the remote may refuse: a record naming one
//! of the attempt's events would wait, durable, for an event no journal holds, and every write
//! built on it with it. So a record of the journal builds on the journal's frontier alone, and an
//! attempt's event on everything emitted, the records of the journal among it.

use std::collections::HashSet;

use beadroll_core::id::EventId;

use super::attempt::Route;

#[derive(Debug)]
pub(super) struct Heads {
    /// The frontier, with every event emitted so far: what an attempt's event builds on.
    all: Vec<EventId>,
    /// The frontier, with the records of the journal emitted so far: what such a record builds on.
    journal: Vec<EventId>,
    /// The attempt's events emitted, by id: none the parent of a create in the journal.
    attempted: HashSet<EventId>,
}

impl Heads {
    /// A transaction's before it emits anything: the frontier, for either route.
    pub(super) fn new(frontier: Vec<EventId>) -> Heads {
        Heads {
            all: frontier.clone(),
            journal: frontier,
            attempted: HashSet::new(),
        }
    }

    /// What an event of `route` builds on: a record of the journal, or an attempt's event.
    pub(super) fn of(&self, route: Route) -> &[EventId] {
        match route {
            Route::Journal => &self.journal,
            Route::Attempt => &self.all,
        }
    }

    /// Whether `id` is an event of the transaction's attempt.
    pub(super) fn attempted(&self, id: EventId) -> bool {
        self.attempted.contains(&id)
    }

    /// After `id` is emitted by `route`, built on [`Heads::of`] it. A record of the journal reaches
    /// every head that is no event of the attempt, which stay heads beside it.
    pub(super) fn emitted(&mut self, id: EventId, route: Route) {
        match route {
            Route::Journal => {
                let attempted = &self.attempted;
                self.all.retain(|head| attempted.contains(head));
                self.all.push(id);
                self.journal = vec![id];
            }
            Route::Attempt => {
                self.all = vec![id];
                self.attempted.insert(id);
            }
        }
    }
}
