//! The guards: the work each algorithm roll-lqjj.15 to .21 made linear does on an input and on one
//! [`LARGER`] times its size, counted, grows with the input and not with its square (X15). Each
//! input is the shape the algorithm was quadratic on before, the shape its `cargo xtask timing`
//! probe measures, built outside the count; only the call is counted. A module for each:
//! `import` (roll-lqjj.15), `finished` (.17), `awaiting` (.18), and `edges`, readiness (.19) and
//! the dependents of the issues shown (.20) with the blockers and readiness of several issues
//! (.24), `degree`, the edges of one issue given many (.21), and `renamed`, the key of a copy two
//! machines minted alike (roll-lqjj.25).
//!
//! Shapes whose cost still grows faster than their input by design are not guarded, and
//! `docs/REVIEWING.md` §2 says why for each: a batch that changes an edge builds the parent-child
//! index again over the whole state (roll-lqjj.17), one issue that waits for N keys that come one
//! batch at a time copies its own growing list (roll-lqjj.18), many kinds of edge on one target are
//! compared with each other (roll-lqjj.21), and a copy of a numbered key
//! (`ex-1.1`), which counts upward past every number taken, so that N copies two machines minted
//! alike ask N² keys (roll-lqjj.46).

use crate::id::EventId;
use crate::journal::Journal;
use crate::json::{Object, Value};
use crate::projection::{Extended, Projection, Record, project};
use crate::replay::replay;
use crate::state::State;

use super::{Work, counted};

mod awaiting;
mod degree;
mod edges;
mod finished;
mod import;
mod renamed;

/// The size of the smaller input.
const SMALL: usize = 250;

/// How many times larger the larger input is.
const LARGER: usize = 8;

/// How many times the work on the smaller input the larger may do: twice what work growing with
/// its input does ([`LARGER`]), where work growing with its square does [`LARGER`]², four times
/// this.
const GROWTH: usize = 2 * LARGER;

/// Asserts that the `work` `call` does on what `build` makes of [`SMALL`] and of [`LARGER`] times
/// that grows with the input: at most [`GROWTH`] times as much on the larger, counting nothing as
/// one.
fn grows_linearly<T>(
    what: &str,
    work: &[Work],
    build: impl Fn(usize) -> T,
    mut call: impl FnMut(T),
) {
    let sizes = [SMALL, LARGER * SMALL];
    let [small, large] = sizes.map(|size| {
        let input = build(size);
        counted(|| call(input)).1
    });
    for &work in work {
        let (small, large) = (small.of(work), large.of(work));
        assert!(
            large <= GROWTH * small.max(1),
            "{what}: {small} of {work:?} on {}, {large} on {}, more than {GROWTH} times as much",
            sizes[0],
            sizes[1],
        );
    }
}

/// The id of event number `n`: a UUIDv7, ids in the order of their numbers.
fn id(n: u64) -> String {
    let n = u128::from(n);
    let value = ((1_790_000_000_000 + n) << 80) | (7 << 76) | (2 << 62) | n;
    EventId::from_u128(value).to_string()
}

/// The machine whose journal every event is in.
const MACHINE: &str = "a";

/// The key of issue `n`.
fn key(n: usize) -> String {
    format!("ex-{n}")
}

/// One machine's journal, each event built on the one before it.
struct History {
    events: Vec<Value>,
}

impl History {
    /// The `init`.
    fn new() -> History {
        let mut history = History { events: Vec::new() };
        let mut data = Object::new();
        data.insert("prefix".into(), Value::from("ex"));
        history.add("init", None, data);
        history
    }

    /// Event number `n`, doing `op` on the issue event `issue` created, or on itself.
    fn add(&mut self, op: &str, issue: Option<u64>, data: Object) -> u64 {
        let n = self.events.len() as u64 + 1;
        let mut event = Object::new();
        event.insert("v".into(), Value::from(1));
        event.insert("id".into(), Value::from(id(n)));
        event.insert("replica".into(), Value::from(MACHINE));
        let deps = (n > 1)
            .then(|| Value::from(id(n - 1)))
            .into_iter()
            .collect();
        event.insert("deps".into(), Value::Array(deps));
        event.insert("op".into(), Value::from(op));
        event.insert("issue".into(), Value::from(id(issue.unwrap_or(n))));
        event.insert("data".into(), Value::from(data));
        self.events.push(Value::from(event));
        n
    }

    /// The create of an issue under `key`, imported with `labels` and a dependency of each type on
    /// each key `linked` names: an edge at once when the key has its issue, waiting otherwise.
    /// Returns its number.
    fn create(&mut self, key: &str, linked: &[(String, &str)], labels: &[&str]) -> u64 {
        let mut fields = Object::new();
        fields.insert("title".into(), Value::from(key));
        let mut data = Object::new();
        data.insert("key".into(), Value::from(key));
        data.insert("fields".into(), Value::from(fields));
        data.insert("imported".into(), Value::from(true));
        if !labels.is_empty() {
            let labels = labels.iter().map(|label| Value::from(*label)).collect();
            data.insert("labels".into(), Value::Array(labels));
        }
        if !linked.is_empty() {
            let deps = (linked.iter())
                .map(|(named, kind)| Value::from(dependency(named, kind)))
                .collect();
            data.insert("dangling".into(), Value::Array(deps));
        }
        self.add("create", None, data)
    }

    /// The journal of every event so far.
    fn journal(&self) -> Journal {
        let mut journal = Journal::new();
        for event in &self.events {
            journal.add(event.clone(), MACHINE, "", true);
        }
        journal
    }

    /// The state replay makes of it.
    fn state(&self) -> State {
        replay(self.journal()).unwrap()
    }

    /// A projection of it.
    fn projection(&self) -> Projection {
        project(self.journal()).unwrap()
    }

    /// The events after the first `held`, as records a projection of those takes in.
    fn after(&self, held: usize) -> Vec<Record> {
        (self.events[held..].iter())
            .map(|value| Record {
                value: value.clone(),
                replica: MACHINE.to_owned(),
                origin: String::new(),
                sequenced: true,
            })
            .collect()
    }
}

/// A dependency on `key` of `kind`, as a record lists one and a create carries one.
fn dependency(key: &str, kind: &str) -> Object {
    let mut dep = Object::new();
    dep.insert("depends_on_id".into(), Value::from(key));
    dep.insert("type".into(), Value::from(kind));
    dep
}

/// `records` applied to `projection` one at a time, as a command that writes one event per issue
/// and reads the state between them does.
fn one_at_a_time(mut projection: Projection, records: Vec<Record>) -> Projection {
    for record in records {
        projection = applied(projection, vec![record]);
    }
    projection
}

/// `records` applied to `projection`, which must take them.
fn applied(projection: Projection, records: Vec<Record>) -> Projection {
    match projection.extend(records).unwrap() {
        Extended::Applied(extended) => *extended,
        Extended::Replay => panic!("the guard's own events are declined"),
    }
}
