//! What a transaction's next event builds on, by the route it takes. Its records of the journal
//! are appended before its attempt is published, which the remote may refuse: a record naming one
//! of the attempt's events would wait, durable, for an event no journal holds, and every write
//! built on it with it. So a record of the journal builds on the journal's frontier alone, and an
//! attempt's event on everything emitted, the records of the journal among it.

use std::collections::HashSet;

use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};

/// The members of an event's data that name an issue by its id: a create's parent, a dependency's
/// target.
const NAMING: [&str; 2] = ["parent", "to"];

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

    /// Whether an event about `issue` with `data` names an event of the transaction's attempt:
    /// as its issue, or in a member of its data that names an issue ([`NAMING`]). A record
    /// of the journal that does builds on nothing that holds it, and replay of a journal whose
    /// attempt landed may take it first, an orphan, or one whose attempt was refused never finds it.
    pub(super) fn names_attempted(&self, issue: Option<&str>, data: &Object) -> bool {
        if self.attempted.is_empty() {
            return false;
        }
        let named =
            |text: &str| EventId::parse(text).is_some_and(|id| self.attempted.contains(&id));
        let member = |key: &str| data.get(key).and_then(Value::as_str);
        issue.is_some_and(named) || NAMING.iter().any(|key| member(key).is_some_and(named))
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
