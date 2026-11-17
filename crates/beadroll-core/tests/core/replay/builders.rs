//! The event builders the replay modules share. Each is kept whole, with the ids it mints: the
//! tests built on one rely on the order of its ids, so each lives in a module of its own.

use std::collections::BTreeSet;

use beadroll_core::id::EventId;
use beadroll_core::journal::{Journal, JournalOrder};
use beadroll_core::json::Value;
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_test_support::events::Event;
use beadroll_test_support::{Json, json};

use super::scoped_damage::id;

pub(crate) fn id_of(event: &Json) -> &str {
    event["id"].as_str().unwrap()
}

pub(crate) fn uid(event: &Json) -> EventId {
    EventId::parse(id_of(event)).unwrap()
}

pub(crate) fn event_id(text: &str) -> EventId {
    EventId::parse(text).unwrap()
}

/// Events numbered by the caller: ids `019a4444-4444-7000-8000-<number>`, in sequenced journals.
pub(crate) mod numbered {
    use super::*;

    /// Event `number`: id `019a4444-4444-7000-8000-<number>`, about `issue` (itself when
    /// `None`).
    pub(crate) fn event(
        number: u64,
        op: &str,
        replica: &str,
        deps: &[&str],
        issue: Option<&str>,
        data: Json,
    ) -> Json {
        let id = format!("019a4444-4444-7000-8000-{number:012x}");
        let event = Event::new(&id, op).replica(replica).deps(deps).data(data);
        match issue {
            Some(issue) => event.issue(issue).json(),
            None => event.json(),
        }
    }

    pub(crate) fn journal(events: &[&Json]) -> Journal {
        let mut journal = Journal::new();
        for event in events {
            let replica = event["replica"].as_str().unwrap().to_owned();
            journal.add((*event).clone(), &replica, "", JournalOrder::Sequenced);
        }
        journal
    }
}

/// Events numbered as a fixture makes them: ids `019a1111-1111-7000-8000-<serial>`, each in its writer's journal, some detached as an attempt file's are.
pub(crate) mod serial {
    use super::*;

    /// Events with ids `019a1111-1111-7000-8000-<serial>`, numbered as they are made.
    pub(crate) struct Fixture {
        serial: u64,
    }

    impl Fixture {
        pub(crate) fn new() -> Fixture {
            Fixture { serial: 0 }
        }

        pub(crate) fn next_id(&mut self) -> String {
            self.serial += 1;
            format!("019a1111-1111-7000-8000-{:012x}", self.serial)
        }

        /// An event about `issue` (itself when `None`), written by `replica`, naming `deps`.
        pub(crate) fn event(
            &mut self,
            op: &str,
            replica: &str,
            deps: &[&str],
            issue: Option<&str>,
            data: Json,
        ) -> Json {
            let id = self.next_id();
            built(&id, op, replica, deps, issue, data)
        }

        /// `init`, and the create of `ex-one`, titled `survivor`.
        pub(crate) fn base(&mut self) -> (Json, Json) {
            let init = self.event("init", "a", &[], None, json!({"prefix": "ex"}));
            let data = json!({"key": "ex-one", "fields": {"title": "survivor"}});
            let create = self.event("create", "a", &[id_of(&init)], None, data);
            (init, create)
        }
    }

    /// Event `id` as the fixture writes it.
    pub(crate) fn built(
        id: &str,
        op: &str,
        replica: &str,
        deps: &[&str],
        issue: Option<&str>,
        data: Json,
    ) -> Json {
        let event = Event::new(id, op).replica(replica).deps(deps).data(data);
        match issue {
            Some(issue) => event.issue(issue).json(),
            None => event.json(),
        }
    }

    /// `event` with the member at `path` set to `value`.
    pub(crate) fn with(event: &Json, path: &[&str], value: Json) -> Json {
        let mut event = event.clone();
        let mut at = &mut event;
        for name in &path[..path.len() - 1] {
            let Json::Object(members) = at else {
                panic!("no object at {name}")
            };
            at = members.get_mut(*name).unwrap();
        }
        let Json::Object(members) = at else {
            panic!("no object to set {path:?} in")
        };
        members.insert(path[path.len() - 1].to_owned(), value);
        event
    }

    /// A settled journal of `events`, each in its writer's journal; the `detached` ones unsequenced, as
    /// an attempt file's events are.
    pub(crate) fn journal(events: &[Json], detached: &BTreeSet<String>) -> Journal {
        let mut journal = Journal::new();
        for event in events {
            let replica = event["replica"].as_str().unwrap().to_owned();
            let sequenced = !detached.contains(id_of(event));
            journal.add(
                event.clone(),
                &replica,
                "",
                if sequenced {
                    JournalOrder::Sequenced
                } else {
                    JournalOrder::Attempt
                },
            );
        }
        journal.settle();
        journal
    }

    pub(crate) fn replayed(events: &[Json]) -> State {
        replay(journal(events, &BTreeSet::new())).unwrap()
    }

    /// The texts of an issue's comments, in order.
    pub(crate) fn texts(state: &State, create: &Json) -> Vec<String> {
        let issue = state.issue(uid(create)).unwrap();
        issue
            .hydrated()
            .unwrap()
            .comments()
            .iter()
            .map(|c| c.get("text").unwrap().as_str().unwrap().to_owned())
            .collect()
    }

    /// The exported id of the comment that says `text`, on the record `key`.
    pub(crate) fn exported_comment(state: &State, key: &str, text: &str) -> Value {
        let records = state.hydrated().unwrap().export_records();
        let record = records
            .iter()
            .find(|r| r["id"].as_str() == Some(key))
            .unwrap();
        let comments = record["comments"].as_array().unwrap();
        comments
            .iter()
            .find(|c| c["text"].as_str() == Some(text))
            .unwrap()["id"]
            .clone()
    }
}

/// Events with an actor, numbered as a fixture makes them: ids `019a3333-3333-7000-8000-<serial>`, each in the journal of the machine named beside it.
pub(crate) mod actors {
    use super::*;

    /// Whose issue an event names: itself, another, or none (no `issue` member at all).
    #[derive(Clone, Copy)]
    pub(crate) enum Target<'a> {
        Itself,
        Named(&'a str),
        Omitted,
    }

    /// Events with fixed UUIDv7s, numbered in the order they are made.
    pub(crate) struct Fixture {
        serial: u64,
    }

    impl Fixture {
        pub(crate) fn new() -> Fixture {
            Fixture { serial: 0 }
        }

        pub(crate) fn event(
            &mut self,
            op: &str,
            replica: &str,
            deps: &[&str],
            issue: Target<'_>,
            data: Json,
        ) -> Json {
            self.serial += 1;
            let eid = format!("019a3333-3333-7000-8000-{:012x}", self.serial);
            let event = Event::new(&eid, op)
                .replica(replica)
                .deps(deps)
                .actor(json!({"agent": "codex", "thread": (format!("{replica}-thread"))}))
                .data(data);
            match issue {
                Target::Itself => event.json(),
                Target::Named(uid) => event.issue(uid).json(),
                Target::Omitted => event.without("issue").json(),
            }
        }

        /// The tracker's init, and one issue, `ex-one`, titled `survivor`.
        pub(crate) fn base(&mut self) -> (Json, Json) {
            let init = self.event("init", "a", &[], Target::Itself, json!({"prefix": "ex"}));
            let create = self.event(
                "create",
                "a",
                &[&id(&init)],
                Target::Itself,
                json!({"key": "ex-one", "fields": {"title": "survivor"}}),
            );
            (init, create)
        }
    }

    /// A journal of `rows`, each event in the journal of the machine named beside it.
    pub(crate) fn add_all(rows: &[(&Json, &str)]) -> Journal {
        let mut journal = Journal::new();
        for (event, physical_replica) in rows {
            journal.add(
                (*event).clone(),
                physical_replica,
                "",
                JournalOrder::Sequenced,
            );
        }
        journal
    }

    pub(crate) fn replayed(rows: &[(&Json, &str)]) -> State {
        replay(add_all(rows)).unwrap()
    }
}
