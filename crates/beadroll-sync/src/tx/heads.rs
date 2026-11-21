//! What a transaction's next event builds on, and whether a record of the journal may still come.
//! Its records of the journal are appended before its ownership attempt is published, which the
//! remote may refuse: a record built on one of the attempt's events would wait, durable, for an
//! event no journal holds, and every write built on it with it; and one built beside them, not on
//! them, could replay before an edit, a claim or a create of the attempt it relies on, its effect
//! lost though both landed. So every record of the journal comes before the attempt's first event,
//! which builds on them all.

use beadroll_core::id::EventId;

use super::attempt::Route;

#[derive(Debug)]
pub(super) struct Heads {
    /// The frontier, with every event emitted so far: what the next event builds on.
    ids: Vec<EventId>,
    /// Whether an event of the attempt was emitted: no record of the journal may follow it.
    attempted: bool,
}

impl Heads {
    /// A transaction's before it emits anything: the frontier.
    pub(super) fn new(frontier: Vec<EventId>) -> Heads {
        Heads {
            ids: frontier,
            attempted: false,
        }
    }

    /// What the next event builds on.
    pub(super) fn ids(&self) -> &[EventId] {
        &self.ids
    }

    /// Whether an event of `route` may be emitted now: a record of the journal only before the
    /// attempt's first event.
    pub(super) fn admits(&self, route: Route) -> bool {
        route == Route::Attempt || !self.attempted
    }

    /// After `id` is emitted by `route`, built on every head.
    pub(super) fn emitted(&mut self, id: EventId, route: Route) {
        self.ids = vec![id];
        self.attempted |= route == Route::Attempt;
    }
}
