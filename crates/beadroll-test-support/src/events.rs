//! Events as bead writes them, for beadroll-core's integration tests:
//! the [`json!`](crate::json) macro, ids that sort in the order they are made, and the envelope
//! `Tx.emit` writes. Core's own unit tests cannot use these: there the dev-dependency would build a
//! second `beadroll_core`, whose types do not match.

use beadroll_core::journal::Journal;
use beadroll_core::journal::JournalOrder;
use beadroll_core::json::Object;
use beadroll_core::replay::replay;
use beadroll_core::state::State;

use crate::Json;

/// A JSON value written as JSON: `json!({"op": "create", "deps": [(id(1))], "n": 5})`. Object keys
/// are string literals; a value of more than one token (a call, a negative number) goes in
/// parentheses.
#[macro_export]
macro_rules! json {
    (null) => { $crate::Json::Null };
    (true) => { $crate::Json::Bool(true) };
    (false) => { $crate::Json::Bool(false) };
    ([]) => { $crate::Json::Array(::std::vec::Vec::new()) };
    ([ $($item:tt),+ $(,)? ]) => { $crate::Json::Array(::std::vec![ $( $crate::json!($item) ),+ ]) };
    ({}) => { $crate::Json::Object(::std::default::Default::default()) };
    ({ $($key:literal : $value:tt),+ $(,)? }) => {
        $crate::Json::Object(::std::boxed::Box::new([ $( (::std::string::String::from($key), $crate::json!($value)) ),+ ].into_iter().collect()))
    };
    ($other:expr) => { $crate::Json::from($other) };
}

/// The id of a test's event number `n`: a UUIDv7 in 2025, ids in the order of their numbers.
pub fn id(n: u64) -> String {
    id_at(1_762_400_000_000, n)
}

/// A UUIDv7 at `millis`, its random bits `n`: ids at one time sort by `n`.
pub fn id_at(millis: u64, n: u64) -> String {
    format!(
        "{:08x}-{:04x}-7{:03x}-{:04x}-{:012x}",
        millis >> 16,
        millis & 0xffff,
        (n >> 62) & 0xfff,
        0x8000 | ((n >> 48) & 0x3fff),
        n & 0xffff_ffff_ffff
    )
}

/// An event as `Tx.emit` writes it, built a member at a time: by default machine `a`, no causes,
/// actor `test` on thread `a`, about itself, with no payload.
#[derive(Clone, Debug)]
pub struct Event(Object);

impl Event {
    /// Event `id` doing `op`.
    pub fn new(id: &str, op: &str) -> Event {
        let mut members = Object::new();
        members.insert("v".into(), Json::from(1));
        members.insert("id".into(), Json::from(id));
        members.insert("replica".into(), Json::from("a"));
        members.insert("deps".into(), Json::Array(Vec::new()));
        members.insert("actor".into(), json!({"agent": "test", "thread": "a"}));
        members.insert("op".into(), Json::from(op));
        members.insert("issue".into(), Json::from(id));
        members.insert("data".into(), Json::from(Object::new()));
        Event(members)
    }

    /// Written by machine `replica`, its actor's thread named after it.
    pub fn replica(mut self, replica: &str) -> Event {
        self.0.insert("replica".into(), Json::from(replica));
        self.0
            .insert("actor".into(), json!({"agent": "test", "thread": replica}));
        self
    }

    /// Naming `deps` as its causes.
    pub fn deps(mut self, deps: &[&str]) -> Event {
        self.0.insert(
            "deps".into(),
            Json::Array(deps.iter().map(|d| Json::from(*d)).collect()),
        );
        self
    }

    /// About issue `uid`.
    pub fn issue(mut self, uid: &str) -> Event {
        self.0.insert("issue".into(), Json::from(uid));
        self
    }

    /// With payload `data`.
    pub fn data(mut self, data: Json) -> Event {
        self.0.insert("data".into(), data);
        self
    }

    /// With actor `actor`.
    pub fn actor(mut self, actor: Json) -> Event {
        self.0.insert("actor".into(), actor);
        self
    }

    /// With member `name` set to `value` (or added after the others).
    pub fn with(mut self, name: &str, value: Json) -> Event {
        self.0.insert(name.into(), value);
        self
    }

    /// Without member `name`.
    pub fn without(mut self, name: &str) -> Event {
        self.0.shift_remove(name);
        self
    }

    /// The event.
    pub fn json(self) -> Json {
        Json::from(self.0)
    }
}

impl From<Event> for Json {
    fn from(event: Event) -> Json {
        event.json()
    }
}

/// The machine that wrote `event`, as a journal file of that machine holds it.
fn replica_of(event: &Json) -> String {
    event
        .get("replica")
        .and_then(Json::as_str)
        .unwrap_or("a")
        .to_owned()
}

/// A journal of `events`, each in its writer's own journal, in order.
pub fn journal(events: &[Json]) -> Journal {
    let mut journal = Journal::new();
    for event in events {
        let replica = replica_of(event);
        journal.add(event.clone(), &replica, "", JournalOrder::Sequenced);
    }
    journal
}

/// The state `events` replay to.
pub fn state(events: &[Json]) -> State {
    replay(journal(events)).expect("a test journal replays")
}
