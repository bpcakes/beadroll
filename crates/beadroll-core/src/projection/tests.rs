//! A projection extended by every batch a store takes in, local writes and merges alike, is the
//! state replay makes of the whole journal, or declines: on histories of machines that write, sync
//! through a remote and race (colliding keys, concurrent edits, claims, takeovers, imports that
//! wait for a key).

use std::collections::{BTreeMap, HashSet};

use crate::id::EventId;
use crate::journal::Journal;
use crate::json::{Object, Value};
use crate::replay::replay;
use crate::state::State;

use super::{Extended, Projection, Record, decode_state, project};

mod hostile;

/// A seeded generator (SplitMix64): the same draws on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        (!items.is_empty()).then(|| &items[self.below(items.len())])
    }
}

const MACHINES: [&str; 3] = ["0000aa01", "0000bb02", "0000cc03"];

/// An event as a store holds it: in its machine's journal, or in an attempt file.
#[derive(Clone)]
struct Written {
    value: Value,
    machine: usize,
    attempt: bool,
}

/// One machine: the events it knows, by index into the world's, and the ones they build on.
#[derive(Default)]
struct Machine {
    known: Vec<usize>,
    named: HashSet<String>,
}

struct World {
    rng: Rng,
    events: Vec<Written>,
    machines: Vec<Machine>,
    remote: Vec<usize>,
    clock: u64,
}

impl World {
    fn new(seed: u64) -> World {
        World {
            rng: Rng(seed),
            events: Vec::new(),
            machines: MACHINES.iter().map(|_| Machine::default()).collect(),
            remote: Vec::new(),
            clock: 1_790_000_000_000,
        }
    }

    fn id_of(&self, at: usize) -> String {
        str_of(self.events[at].value.get("id"))
    }

    /// What machine `m` builds on: the events it knows that none it knows names.
    fn heads(&self, m: usize) -> Vec<String> {
        let mut heads: Vec<String> = (self.machines[m].known.iter())
            .map(|at| self.id_of(*at))
            .filter(|id| !self.machines[m].named.contains(id))
            .collect();
        heads.sort();
        heads
    }

    fn learn(&mut self, m: usize, at: usize) {
        if self.machines[m].known.contains(&at) {
            return;
        }
        let deps = match self.events[at].value.get("deps") {
            Some(Value::Array(deps)) => deps.iter().map(|d| str_of(Some(d))).collect(),
            _ => Vec::new(),
        };
        self.machines[m].known.push(at);
        self.machines[m].named.extend(deps);
    }

    /// Machine `m` writes `op` on `issue` (itself, for a create): an attempt file's for a claim
    /// or a release, its journal's otherwise. Returns the event's id.
    fn write(&mut self, m: usize, op: &str, issue: Option<String>, data: Object) -> String {
        self.clock += 1 + self.rng.below(3) as u64;
        let random = u128::from(self.rng.next()) & ((1 << 62) - 1);
        let value = (u128::from(self.clock) << 80) | (7 << 76) | (2 << 62) | random;
        let id = EventId::from_u128(value).to_string();
        let mut event = Object::new();
        event.insert("v".into(), Value::from(1));
        event.insert("id".into(), Value::from(id.as_str()));
        event.insert("replica".into(), Value::from(MACHINES[m]));
        let deps = self.heads(m).into_iter().map(Value::from).collect();
        event.insert("deps".into(), Value::Array(deps));
        let mut actor = Object::new();
        actor.insert("agent".into(), Value::from("claude"));
        actor.insert(
            "thread".into(),
            Value::from(format!("t{}", self.rng.below(2))),
        );
        event.insert("actor".into(), Value::from(actor));
        event.insert("op".into(), Value::from(op));
        event.insert(
            "issue".into(),
            Value::from(issue.unwrap_or_else(|| id.clone())),
        );
        event.insert("data".into(), Value::from(data));
        let attempt = matches!(op, "claim" | "release");
        self.events.push(Written {
            value: Value::from(event),
            machine: m,
            attempt,
        });
        let at = self.events.len() - 1;
        self.learn(m, at);
        id
    }

    /// Machine `m` publishes what it knows and takes in what the remote has.
    fn sync(&mut self, m: usize) -> Vec<usize> {
        for at in self.machines[m].known.clone() {
            if !self.remote.contains(&at) {
                self.remote.push(at);
            }
        }
        let new: Vec<usize> = (self.remote.iter())
            .filter(|at| !self.machines[m].known.contains(at))
            .copied()
            .collect();
        for at in &new {
            self.learn(m, *at);
        }
        new
    }

    /// The issues machine `m` knows of: their ids, and the claims on them it knows.
    fn issues(&self, m: usize) -> (Vec<String>, Vec<(String, String)>) {
        let mut issues = Vec::new();
        let mut claims = Vec::new();
        for at in &self.machines[m].known {
            let value = &self.events[*at].value;
            match value.get("op").and_then(Value::as_str) {
                Some("create") => issues.push(self.id_of(*at)),
                Some("claim") => claims.push((str_of(value.get("issue")), self.id_of(*at))),
                _ => {}
            }
        }
        (issues, claims)
    }

    /// One thing machine `m` does.
    fn act(&mut self, m: usize) {
        let (issues, claims) = self.issues(m);
        let r = &mut self.rng;
        let issue = r.pick(&issues).cloned();
        let claim = r.pick(&claims).cloned();
        let roll = r.below(100);
        let label = || Value::Array(vec![Value::from(["ui", "db", "ops"][roll % 3])]);
        let mut data = Object::new();
        if issue.is_none() || roll < 18 {
            let key = format!("pr-{}", ["a1", "b2", "c3", "d4", "e5"][self.rng.below(5)]);
            data.insert("key".into(), Value::from(key));
            let mut fields = Object::new();
            fields.insert("title".into(), Value::from(format!("t{roll}")));
            fields.insert("status".into(), Value::from("open"));
            fields.insert("priority".into(), Value::from(2));
            fields.insert("issue_type".into(), Value::from("task"));
            data.insert("fields".into(), Value::from(fields));
            if self.rng.chance(20) {
                let mut dep = Object::new();
                let key = format!("pr-{}", ["a1", "b2", "f6"][self.rng.below(3)]);
                dep.insert("depends_on_id".into(), Value::from(key));
                dep.insert("type".into(), Value::from("blocks"));
                data.insert("dangling".into(), Value::Array(vec![Value::from(dep)]));
                data.insert("imported".into(), Value::from(true));
            }
            self.write(m, "create", None, data);
            return;
        }
        let (op, data) = match roll {
            18..=34 => {
                let mut fields = Object::new();
                let field = ["title", "assignee", "description"][roll % 3];
                fields.insert(field.into(), Value::from(format!("v{roll}")));
                data.insert("fields".into(), Value::from(fields));
                ("set", data)
            }
            35..=44 => {
                data.insert("text".into(), Value::from(format!("c{roll}")));
                ("comment", data)
            }
            45..=54 => {
                data.insert("labels".into(), label());
                (["label_add", "label_remove", "label_set"][roll % 3], data)
            }
            55..=66 => {
                let to = self.rng.pick(&issues).cloned().unwrap_or_default();
                data.insert("to".into(), Value::from(to));
                data.insert(
                    "type".into(),
                    Value::from(["blocks", "parent-child"][roll % 2]),
                );
                if roll.is_multiple_of(4) {
                    ("dep_remove", data)
                } else {
                    data.insert("meta".into(), Value::from(Object::new()));
                    ("dep_add", data)
                }
            }
            67..=69 => {
                data.insert("key".into(), Value::from("pr-a1"));
                ("dep_remove", data)
            }
            70..=79 => {
                let offline = roll.is_multiple_of(4);
                data.insert(
                    "mode".into(),
                    Value::from(if offline { "offline" } else { "online" }),
                );
                if roll == 79
                    && let Some((on, held)) = &claim
                {
                    data.insert("takeover".into(), Value::from(held.as_str()));
                    data.insert("reason".into(), Value::from("gone"));
                    return drop(self.write(m, "claim", Some(on.clone()), data));
                }
                ("claim", data)
            }
            80..=86 => match &claim {
                Some((on, held)) => {
                    data.insert("claim".into(), Value::from(held.as_str()));
                    return drop(self.write(m, "release", Some(on.clone()), data));
                }
                None => ("reopen", data),
            },
            87..=93 => {
                if let Some((_, held)) = claim.filter(|_| roll.is_multiple_of(2)) {
                    data.insert("claim".into(), Value::from(held));
                }
                data.insert("reason".into(), Value::from("done"));
                ("close", data)
            }
            94..=97 => ("reopen", data),
            _ => ("delete", data),
        };
        self.write(m, op, issue, data);
    }
}

fn str_of(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// The store of machine 0 as a list of what it took in, in order: its records, as a store's files
/// hold them, a batch at a time.
fn records(world: &World, batch: &[usize]) -> Vec<Record> {
    batch
        .iter()
        .map(|at| {
            let written = &world.events[*at];
            let machine = MACHINES[written.machine];
            let id = world.id_of(*at);
            let origin = if written.attempt {
                format!("events/{machine}/claims/{id}.json")
            } else {
                format!("events/{machine}/000001.jsonl")
            };
            Record {
                value: written.value.clone(),
                replica: machine.to_owned(),
                origin,
                sequenced: !written.attempt,
            }
        })
        .collect()
}

/// The journal a store holding `held` loads: every machine's records in its journal's order, then
/// the attempt files, by name.
fn load(world: &World, held: &[usize]) -> Journal {
    let mut journal = Journal::new();
    let mut attempts: BTreeMap<(usize, String), usize> = BTreeMap::new();
    for m in 0..MACHINES.len() {
        for at in held.iter().filter(|at| world.events[**at].machine == m) {
            if world.events[*at].attempt {
                attempts.insert((m, world.id_of(*at)), *at);
                continue;
            }
            for record in records(world, &[*at]) {
                journal.add(record.value, &record.replica, &record.origin, true);
            }
        }
    }
    for at in attempts.into_values() {
        for record in records(world, &[at]) {
            journal.add(record.value, &record.replica, &record.origin, false);
        }
    }
    journal
}

/// Whether two states answer alike: everything replay builds, and what a state answers with of its
/// events (who wrote what a register holds, where machines' journals broke).
fn same(a: &State, b: &State) -> bool {
    let conflicts = |st: &State| -> Vec<String> {
        (st.issues.values())
            .map(|issue| format!("{:?}", st.conflicts(issue)))
            .collect()
    };
    conflicts(a) == conflicts(b)
        && a.chain_breaks() == b.chain_breaks()
        && a.prefix == b.prefix
        && a.name == b.name
        && a.issues.iter().eq(b.issues.iter())
        && a.by_key.iter().eq(b.by_key.iter())
        && a.renames == b.renames
        && a.integrity == b.integrity
        && a.pending == b.pending
        && a.orphans == b.orphans
        && a.comment_count == b.comment_count
        && a.last_by_thread.iter().eq(b.last_by_thread.iter())
        && a.children == b.children
        && a.awaiting == b.awaiting
}

impl World {
    /// What machine 0's store takes in next, a local write's events or a merge's, after the other
    /// machines have done what they do meanwhile.
    fn next_batch(&mut self) -> Vec<usize> {
        loop {
            let m = self.rng.below(MACHINES.len());
            if m == 0 && self.rng.chance(60) {
                let before = self.events.len();
                for _ in 0..1 + self.rng.below(3) {
                    self.act(0);
                }
                return (before..self.events.len()).collect();
            }
            if m == 0 {
                return self.sync(0);
            }
            // Another machine usually takes in the latest before it writes, and publishes after.
            if self.rng.chance(70) {
                self.sync(m);
            }
            for _ in 0..1 + self.rng.below(3) {
                self.act(m);
            }
            self.sync(m);
        }
    }

    /// Whether `batch` holds a takeover.
    fn takes_over(&self, batch: &[usize]) -> bool {
        (batch.iter()).any(|at| {
            let data = self.events[*at].value.get("data");
            data.is_some_and(|data| data.get("takeover").is_some())
        })
    }
}

/// The projection read back from its bytes, as the next command reads it.
fn read_back(projection: &Projection) -> Projection {
    let encoded = projection.encode();
    let decoded = Projection::decode(&encoded).unwrap();
    assert!(same(decoded.state(), projection.state()));
    assert!(same(
        &decode_state(&encoded.state).unwrap(),
        projection.state()
    ));
    decoded
}

/// Whether `extended` is what projecting the whole journal (`full`) makes.
fn equal(extended: &Projection, full: &Projection) -> bool {
    same(extended.state(), full.state())
        && extended.heads == full.heads
        && extended.ids == full.ids
        && extended.lengths == full.lengths
        && extended.open == full.open
}

#[test]
fn every_extension_is_replays_answer_or_declined() {
    // Merges taken in, and local writes the projection declined: only a takeover is declined.
    let (mut merged, mut declined) = (0, Vec::new());
    for seed in 0..40 {
        let mut world = World::new(20_261_012 + seed);
        let mut data = Object::new();
        data.insert("prefix".into(), Value::from("pr"));
        world.write(0, "init", None, data);
        let mut held: Vec<usize> = vec![0];
        let mut projection = project(load(&world, &held)).unwrap();
        for _ in 0..60 {
            let batch = world.next_batch();
            if world.rng.chance(50) {
                projection = read_back(&projection);
            }
            held.extend(&batch);
            let full = project(load(&world, &held)).unwrap();
            assert!(same(full.state(), &replay(load(&world, &held)).unwrap()));
            let local = batch.iter().all(|at| world.events[*at].machine == 0);
            projection = match projection.extend(records(&world, &batch)).unwrap() {
                Extended::Applied(extended) => {
                    assert!(equal(&extended, &full), "seed {seed}");
                    merged += usize::from(!local);
                    *extended
                }
                Extended::Replay => {
                    if local && !world.takes_over(&batch) {
                        declined.push(seed);
                    }
                    full
                }
            };
        }
    }
    // A local write builds on the frontier: the projection takes it, a takeover's excepted. A
    // merge it takes when the merged events build on it.
    assert!(
        declined.is_empty(),
        "local writes declined in seeds {declined:?}"
    );
    assert!(merged > 50, "{merged} merges taken in");
}
