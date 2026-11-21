//! The world the differential tests draw histories from: machines that write, sync through a
//! remote and race, each event as a store holds it, and the journal machine 0's store loads.

use std::collections::{BTreeMap, HashSet};

use super::fields::{CreateOrigin, created, written};
use super::{Causes, HistoryKind, Rng};
use crate::event::event_problem;
use crate::id::EventId;
use crate::journal::{Journal, JournalOrder, canonical};
use crate::json::{Object, Value};
use crate::projection::Record;

pub(super) const MACHINES: [&str; 3] = ["0000aa01", "0000bb02", "0000cc03"];

/// An event as a store holds it: in its machine's journal, or in an attempt file.
#[derive(Clone)]
pub(super) struct Written {
    pub(super) value: Value,
    pub(super) machine: usize,
    pub(super) attempt: bool,
}

/// One machine: the events it knows, by index into the world's.
#[derive(Default)]
pub(super) struct Machine {
    pub(super) known: Vec<usize>,
}

pub(super) struct World {
    pub(super) rng: Rng,
    pub(super) events: Vec<Written>,
    pub(super) machines: Vec<Machine>,
    /// The folder each machine writes its journal to: a machine whose identity was copied shares
    /// it with the one it was copied from.
    pub(super) folders: Vec<&'static str>,
    pub(super) remote: Vec<usize>,
    pub(super) clock: u64,
    /// Whether the machines other than machine 0 also write what replay reports ([`World::flaw`]).
    pub(super) flawed: bool,
    /// The id the next event written gets, when one was named before it was written.
    pub(super) next_id: Option<String>,
    /// Ids a takeover named before any event had them, with the issue it was on.
    pub(super) promised: Vec<(String, String)>,
    /// Whether machine 0 builds on what its projection names (`Projection::causes_for`), and the
    /// machines also write what keeps the frontier off a journal's last record.
    pub(super) chained: bool,
    /// Machine 0's writes built so, by index, with what they were built on: for the test to judge.
    pub(super) built: Vec<(usize, Causes)>,
    /// Ids events name as causes that no event has yet, for another machine to write later.
    pub(super) owed: Vec<String>,
    /// Whether the machines also write cycles of causes, which replay never applies.
    pub(super) cycles: bool,
}

impl World {
    pub(super) fn new(seed: u64) -> World {
        World {
            rng: Rng::new(seed),
            events: Vec::new(),
            machines: MACHINES.iter().map(|_| Machine::default()).collect(),
            folders: MACHINES.to_vec(),
            remote: Vec::new(),
            clock: 1_790_000_000_000,
            flawed: false,
            next_id: None,
            promised: Vec::new(),
            chained: false,
            built: Vec::new(),
            owed: Vec::new(),
            cycles: false,
        }
    }

    pub(super) fn id_of(&self, at: usize) -> String {
        str_of(self.events[at].value.get("id"))
    }

    /// What machine `m` builds on: the events it knows that none it knows names, its records
    /// settled into events as a journal settles them. An id's own record is a valid event in the
    /// journal of the machine it names. An id found with several contents is an event, built on
    /// what its first own record names (nothing, without one); one with a single content is, when
    /// its record is own or not a valid event, built on what that record names; a valid record
    /// found only in another machine's journal is no event.
    pub(super) fn heads(&self, m: usize) -> Vec<String> {
        let mut records: BTreeMap<String, Vec<&Written>> = BTreeMap::new();
        for at in &self.machines[m].known {
            records
                .entry(self.id_of(*at))
                .or_default()
                .push(&self.events[*at]);
        }
        let mut named = HashSet::new();
        let mut events = Vec::new();
        for (id, records) in records.iter().filter(|(id, _)| !id.is_empty()) {
            let valid = |written: &&&Written| event_problem(&written.value).is_none();
            let own = (records.iter()).find(|written| {
                valid(written)
                    && str_of(written.value.get("replica")) == self.folders[written.machine]
            });
            let contents: HashSet<String> = (records.iter())
                .map(|written| canonical(&written.value))
                .collect();
            let builds_on = match (own, records.first()) {
                (Some(own), _) => Some(own),
                (None, _) if contents.len() > 1 => None,
                (None, Some(first)) if !valid(&first) => Some(first),
                _ => continue,
            };
            events.push(id.clone());
            if let Some(Value::Array(deps)) = builds_on.and_then(|on| on.value.get("deps")) {
                named.extend(deps.iter().map(|dep| str_of(Some(dep))));
            }
        }
        events.retain(|id| !named.contains(id));
        events
    }

    pub(super) fn learn(&mut self, m: usize, at: usize) {
        if !self.machines[m].known.contains(&at) {
            self.machines[m].known.push(at);
        }
    }

    /// Machine `m` writes `op` on `issue` (itself, for a create): an attempt file's for a claim
    /// or a release, its journal's otherwise. Returns the event's id.
    pub(super) fn write(
        &mut self,
        m: usize,
        op: &str,
        issue: Option<String>,
        data: Object,
    ) -> String {
        self.write_as(m, None, op, issue, data)
    }

    /// [`World::write`], by the agent's thread `thread` when given, and one drawn otherwise.
    pub(super) fn write_as(
        &mut self,
        m: usize,
        thread: Option<&str>,
        op: &str,
        issue: Option<String>,
        data: Object,
    ) -> String {
        self.clock += 1 + self.rng.below(3) as u64;
        let id = match self.next_id.take() {
            Some(id) => id,
            None => self.mint(),
        };
        let mut event = Object::new();
        event.insert("v".into(), Value::from(1));
        event.insert("id".into(), Value::from(id.as_str()));
        event.insert("replica".into(), Value::from(self.folders[m]));
        let attempt = matches!(op, "claim" | "release");
        let causes = (self.chained && m == 0).then(|| {
            let known = self.machines[0].known.clone();
            crate::projection::project(load(self, &known))
                .unwrap()
                .causes_for(MACHINES[0])
        });
        let deps = match &causes {
            // as a transaction builds: the machine's last record too, for one of its journal
            Some(causes) => (causes.ids.iter())
                .chain(causes.predecessor.iter().filter(|_| !attempt))
                .map(|id| Value::from(id.to_string()))
                .collect(),
            None => self.heads(m).into_iter().map(Value::from).collect(),
        };
        event.insert("deps".into(), Value::Array(deps));
        let mut actor = Object::new();
        actor.insert("agent".into(), Value::from("claude"));
        let thread = match thread {
            Some(thread) => thread.to_owned(),
            None => format!("t{}", self.rng.below(2)),
        };
        actor.insert("thread".into(), Value::from(thread));
        event.insert("actor".into(), Value::from(actor));
        event.insert("op".into(), Value::from(op));
        event.insert(
            "issue".into(),
            Value::from(issue.unwrap_or_else(|| id.clone())),
        );
        event.insert("data".into(), Value::from(data));
        self.events.push(Written {
            value: Value::from(event),
            machine: m,
            attempt,
        });
        let at = self.events.len() - 1;
        self.learn(m, at);
        self.built.extend(causes.map(|causes| (at, causes)));
        id
    }

    /// A new event id, of the world's clock.
    pub(super) fn mint(&mut self) -> String {
        let random = u128::from(self.rng.bits()) & ((1 << 62) - 1);
        let value = (u128::from(self.clock) << 80) | (7 << 76) | (2 << 62) | random;
        EventId::from_u128(value).to_string()
    }

    /// Machine `m` publishes what it knows and takes in what the remote has.
    pub(super) fn sync(&mut self, m: usize) -> Vec<usize> {
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
    pub(super) fn issues(&self, m: usize) -> (Vec<String>, Vec<(String, String)>) {
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

    /// Creates an issue, optionally with imported fields and unresolved dependencies.
    pub(super) fn create_issue(&mut self, m: usize, roll: usize, mut data: Object) {
        let key = format!("pr-{}", ["a1", "b2", "c3", "d4", "e5"][self.rng.below(5)]);
        data.insert("key".into(), Value::from(key));
        let origin = if self.rng.chance(20) {
            CreateOrigin::Imported
        } else {
            CreateOrigin::Local
        };
        let fields = created(&mut self.rng, roll, origin);
        data.insert("fields".into(), Value::from(fields));
        if origin == CreateOrigin::Imported {
            let mut dep = Object::new();
            let key = format!("pr-{}", ["a1", "b2", "f6"][self.rng.below(3)]);
            dep.insert("depends_on_id".into(), Value::from(key));
            dep.insert("type".into(), Value::from("blocks"));
            let mut dangling = vec![Value::from(dep)];
            if self.rng.chance(50) {
                dangling.extend(self.awaited(roll));
            }
            data.insert("dangling".into(), Value::Array(dangling));
            data.insert("imported".into(), Value::from(true));
        }
        self.write(m, "create", None, data);
    }

    /// One thing machine `m` does.
    pub(super) fn act(&mut self, m: usize) {
        let (issues, claims) = self.issues(m);
        let r = &mut self.rng;
        let issue = r.pick(&issues).cloned();
        let claim = r.pick(&claims).cloned();
        let roll = r.below(100);
        let label = || Value::Array(vec![Value::from(["ui", "db", "ops"][roll % 3])]);
        let mut data = Object::new();
        if issue.is_none() || roll < 18 {
            self.create_issue(m, roll, data);
            return;
        }
        // who holds an issue, half the time drawn as a world heavy in ownership draws it
        if let Some(issue) = issue.clone().filter(|_| roll >= 70 && self.rng.chance(50)) {
            return self.own(m, issue);
        }
        let (op, data) = match roll {
            18..=34 => {
                data.insert("fields".into(), Value::from(written(&mut self.rng, roll)));
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
            55..=66 if self.rng.chance(40) => {
                let issue = issue.unwrap_or_default();
                let (op, on, data) = self.edge_drawn(m, issue, &issues, roll);
                return drop(self.write(m, op, Some(on), data));
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

pub(super) fn str_of(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// The store of machine 0 as a list of what it took in, in order: its records, as a store's files
/// hold them, a batch at a time.
pub(super) fn records(world: &World, batch: &[usize]) -> Vec<Record> {
    batch
        .iter()
        .map(|at| {
            let written = &world.events[*at];
            let machine = world.folders[written.machine];
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
pub(super) fn load(world: &World, held: &[usize]) -> Journal {
    let mut journal = Journal::new();
    let mut attempts: BTreeMap<(usize, String), usize> = BTreeMap::new();
    for (m, folder) in MACHINES.iter().enumerate() {
        let here = |at: &&usize| world.folders[world.events[**at].machine] == *folder;
        for at in held.iter().filter(here) {
            if world.events[*at].attempt {
                attempts.insert((m, world.id_of(*at)), *at);
                continue;
            }
            for record in records(world, &[*at]) {
                journal.add(
                    record.value,
                    &record.replica,
                    &record.origin,
                    JournalOrder::Sequenced,
                );
            }
        }
    }
    for at in attempts.into_values() {
        for record in records(world, &[at]) {
            journal.add(
                record.value,
                &record.replica,
                &record.origin,
                JournalOrder::Attempt,
            );
        }
    }
    journal
}

/// Every record the machines of the world of `seed` write while machine 0's store takes in
/// `batches` batches, in the order they were written, each as a store holds it: the histories the
/// properties of replay deliver in other orders (`replay::tests`). A world `flawed` also writes
/// what replay reports.
pub(crate) fn history(seed: u64, batches: usize, flawed: HistoryKind) -> Vec<Record> {
    let mut world = World::of(seed, flawed);
    let mut data = Object::new();
    data.insert("prefix".into(), Value::from("pr"));
    world.write(0, "init", None, data);
    for _ in 0..batches {
        world.next_batch();
    }
    let every: Vec<usize> = (0..world.events.len()).collect();
    records(&world, &every)
}

impl World {
    /// What machine 0's store takes in next, a local write's events or a merge's, after the other
    /// machines have done what they do meanwhile.
    pub(super) fn next_batch(&mut self) -> Vec<usize> {
        loop {
            let m = self.rng.below(self.machines.len());
            if m == 0 && self.rng.chance(60) {
                let before = self.events.len();
                for _ in 0..1 + self.rng.below(3) {
                    if self.chained && self.rng.chance(10) {
                        self.chain_flaw(0);
                    } else {
                        self.act(0);
                    }
                }
                return (before..self.events.len()).collect();
            }
            if m == 0 {
                return self.sync(0);
            }
            // a copied identity writes now and then, and breaks its journal's chain each time
            if self.flawed && m == 3 && !self.rng.chance(20) {
                continue;
            }
            // and a copy of machine 0's, seldom
            if m == 4 && !self.rng.chance(5) {
                continue;
            }
            // two machines now and then claim one issue at once
            if self.rng.chance(10) {
                self.race();
                continue;
            }
            self.pays(m);
            // Another machine usually takes in the latest before it writes, and publishes after.
            if self.rng.chance(70) {
                self.sync(m);
            }
            for _ in 0..1 + self.rng.below(3) {
                if self.chained && self.rng.chance(6) {
                    self.chain_flaw(m);
                } else if self.flawed && self.rng.chance(15) {
                    self.flaw(m);
                } else {
                    self.act(m);
                }
            }
            self.sync(m);
        }
    }

    /// Whether `batch` holds a takeover.
    pub(super) fn takes_over(&self, batch: &[usize]) -> bool {
        (batch.iter()).any(|at| {
            let data = self.events[*at].value.get("data");
            data.is_some_and(|data| data.get("takeover").is_some())
        })
    }
}
