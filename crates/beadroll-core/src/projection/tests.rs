//! A projection extended by every batch a store takes in, local writes and merges alike, is the
//! state replay makes of the whole journal, or declines: on histories of machines that write, sync
//! through a remote and race (colliding keys, concurrent edits, claims, takeovers, imports that
//! wait for a key).

use std::collections::{BTreeMap, HashSet};

use crate::event::event_problem;
use crate::id::EventId;
use crate::journal::{Journal, canonical};
use crate::json::{Object, Value};
use crate::state::State;

use super::parts::Copying;
use super::{Projection, Record, decode_state, sum, sum_of};

pub(crate) mod awaiting;
mod compacted;
mod copied;
mod edges;
mod finished;
mod flawed;
mod forged;
mod hostile;
mod known;
mod marked;
pub(crate) mod owned;
mod reported;
mod shown;
mod taken;
mod texts_once;

use taken::differential;

/// A seeded generator (SplitMix64): the same draws on every run.
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub(crate) fn below(&mut self, n: usize) -> usize {
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

/// One machine: the events it knows, by index into the world's.
#[derive(Default)]
struct Machine {
    known: Vec<usize>,
}

struct World {
    rng: Rng,
    events: Vec<Written>,
    machines: Vec<Machine>,
    /// The folder each machine writes its journal to: a machine whose identity was copied shares
    /// it with the one it was copied from.
    folders: Vec<&'static str>,
    remote: Vec<usize>,
    clock: u64,
    /// Whether the machines other than machine 0 also write what replay reports ([`World::flaw`]).
    flawed: bool,
    /// The id the next event written gets, when one was named before it was written.
    next_id: Option<String>,
    /// Ids a takeover named before any event had them, with the issue it was on.
    promised: Vec<(String, String)>,
}

impl World {
    fn new(seed: u64) -> World {
        World {
            rng: Rng(seed),
            events: Vec::new(),
            machines: MACHINES.iter().map(|_| Machine::default()).collect(),
            folders: MACHINES.to_vec(),
            remote: Vec::new(),
            clock: 1_790_000_000_000,
            flawed: false,
            next_id: None,
            promised: Vec::new(),
        }
    }

    fn id_of(&self, at: usize) -> String {
        str_of(self.events[at].value.get("id"))
    }

    /// What machine `m` builds on: the events it knows that none it knows names, its records
    /// settled into events as a journal settles them. An id's own record is a valid event in the
    /// journal of the machine it names. An id found with several contents is an event, built on
    /// what its first own record names (nothing, without one); one with a single content is, when
    /// its record is own or not a valid event, built on what that record names; a valid record
    /// found only in another machine's journal is no event.
    fn heads(&self, m: usize) -> Vec<String> {
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

    fn learn(&mut self, m: usize, at: usize) {
        if !self.machines[m].known.contains(&at) {
            self.machines[m].known.push(at);
        }
    }

    /// Machine `m` writes `op` on `issue` (itself, for a create): an attempt file's for a claim
    /// or a release, its journal's otherwise. Returns the event's id.
    fn write(&mut self, m: usize, op: &str, issue: Option<String>, data: Object) -> String {
        self.write_as(m, None, op, issue, data)
    }

    /// [`World::write`], by the agent's thread `thread` when given, and one drawn otherwise.
    fn write_as(
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
        let deps = self.heads(m).into_iter().map(Value::from).collect();
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

    /// A new event id, of the world's clock.
    fn mint(&mut self) -> String {
        let random = u128::from(self.rng.next()) & ((1 << 62) - 1);
        let value = (u128::from(self.clock) << 80) | (7 << 76) | (2 << 62) | random;
        EventId::from_u128(value).to_string()
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
            let imported = self.rng.chance(20);
            let fields = created(&mut self.rng, roll, imported);
            data.insert("fields".into(), Value::from(fields));
            if imported {
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

/// The fields of a generated create, which now and then leaves a field to its default, gives one
/// null or a legacy boolean priority, or makes an issue closed with what it gives of the close;
/// an import's, now and then the times and the author its record gives, the author null included.
fn created(r: &mut Rng, roll: usize, imported: bool) -> Object {
    let mut fields = Object::new();
    for field in ["title", "priority", "issue_type"] {
        if !r.chance(10) {
            fields.insert(field.into(), drawn(r, field, roll, false));
        }
    }
    match r.below(10) {
        0 => {}
        1 => {
            fields.insert("status".into(), Value::from("closed"));
            for field in ["closed_at", "close_reason"] {
                let value = match r.below(3) {
                    0 => continue,
                    1 => Value::Null,
                    _ => drawn(r, field, roll, false),
                };
                fields.insert(field.into(), value);
            }
        }
        _ => {
            fields.insert("status".into(), Value::from("open"));
        }
    }
    for field in ["assignee", "estimated_minutes", "external_ref"] {
        if r.chance(40) {
            fields.insert(field.into(), drawn(r, field, roll, true));
        }
    }
    for field in ["created_at", "updated_at", "created_by"] {
        if imported && r.chance(50) {
            let value = match field {
                "created_by" if r.chance(30) => Value::Null,
                "created_by" => Value::from(format!("v{roll}")),
                _ => Value::from(format!("2025-12-{:02}T00:00:00Z", 1 + r.below(28))),
            };
            fields.insert(field.into(), value);
        }
    }
    fields
}

/// The fields of a generated set: one to three, in the order drawn, so that a status comes before
/// or after a closed_at or close_reason, which a set writes into registers of their own.
fn written(r: &mut Rng, roll: usize) -> Object {
    let mut fields = Object::new();
    for _ in 0..=r.below(3) {
        let field = *r.pick(&SET).expect("fields");
        if !fields.contains_key(field) {
            let value = drawn(r, field, roll, true);
            fields.insert(field.into(), value);
        }
    }
    fields
}

/// The fields a generated set writes.
const SET: [&str; 10] = [
    "title",
    "assignee",
    "description",
    "priority",
    "estimated_minutes",
    "external_ref",
    "status",
    "closed_at",
    "close_reason",
    "notes",
];

/// A value of `field` for an event of `roll` to write: null now and then where `nullable` and the
/// field may be cleared, and a legacy boolean priority now and then.
fn drawn(r: &mut Rng, field: &str, roll: usize, nullable: bool) -> Value {
    let required = matches!(field, "title" | "priority" | "issue_type" | "status");
    if nullable && !required && r.chance(30) {
        return Value::Null;
    }
    match field {
        "priority" if r.chance(15) => Value::Bool(r.chance(50)),
        "priority" => Value::from(r.below(5) as i64),
        "estimated_minutes" => Value::from(roll as i64),
        "issue_type" => Value::from(["task", "bug"][r.below(2)]),
        "status" => Value::from(["open", "in_progress", "closed"][r.below(3)]),
        "closed_at" => Value::from(format!("2026-01-{:02}T00:00:00Z", 1 + roll % 28)),
        _ => Value::from(format!("v{roll}")),
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
fn load(world: &World, held: &[usize]) -> Journal {
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

/// Every record the machines of the world of `seed` write while machine 0's store takes in
/// `batches` batches, in the order they were written, each as a store holds it: the histories the
/// properties of replay deliver in other orders (`replay::tests`). A world `flawed` also writes
/// what replay reports.
pub(crate) fn history(seed: u64, batches: usize, flawed: bool) -> Vec<Record> {
    let mut world = if flawed {
        World::flawed(seed)
    } else {
        World::new(seed)
    };
    let mut data = Object::new();
    data.insert("prefix".into(), Value::from("pr"));
    world.write(0, "init", None, data);
    for _ in 0..batches {
        world.next_batch();
    }
    let every: Vec<usize> = (0..world.events.len()).collect();
    records(&world, &every)
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
        && awaiting(a).is_some_and(|listed| Some(listed) == awaiting(b))
}

/// What `state` lists as waiting, by place: where an entry is listed at a place that does not hold
/// it, that place is `None`, which no state agrees with.
fn awaiting(state: &State) -> Option<Vec<(String, EventId, usize)>> {
    (state.awaiting.listed(&state.issues).into_iter())
        .map(|(key, uid, at)| Some((key, uid, at?)))
        .collect()
}

impl World {
    /// What machine 0's store takes in next, a local write's events or a merge's, after the other
    /// machines have done what they do meanwhile.
    fn next_batch(&mut self) -> Vec<usize> {
        loop {
            let m = self.rng.below(self.machines.len());
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
            // a copied identity writes now and then, and breaks its journal's chain each time
            if self.flawed && m == 3 && !self.rng.chance(20) {
                continue;
            }
            // two machines now and then claim one issue at once
            if self.rng.chance(10) {
                self.race();
                continue;
            }
            // Another machine usually takes in the latest before it writes, and publishes after.
            if self.rng.chance(70) {
                self.sync(m);
            }
            for _ in 0..1 + self.rng.below(3) {
                if self.flawed && self.rng.chance(15) {
                    self.flaw(m);
                } else {
                    self.act(m);
                }
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

/// What a store's text file holds: the texts every projection read back placed, appended.
type TextFile = Vec<u8>;

/// Reads into `state` the texts it has there of `uids`, or of every issue, from `file`, as a store
/// reads them in.
fn read_in(state: &mut State, file: &[u8], uids: Option<Vec<EventId>>) {
    let wanted = match uids {
        Some(uids) => state.wanted_texts(uids),
        None => state.all_wanted_texts(),
    };
    for (uid, place) in wanted {
        let at = usize::try_from(place.at).unwrap();
        let bytes = &file[at..at + place.len as usize];
        state.read_texts(uid, place, bytes).unwrap();
    }
}

/// `projection` with every text it has there read in from `file`.
fn whole(projection: &Projection, file: &[u8]) -> Projection {
    let mut whole = projection.clone();
    read_in(whole.state_mut(), file, None);
    whole
}

/// How a track of the differential tests writes its projection back (roll-lqjj.43): as
/// [`Projection::encode`] does, or at every write-back with what every event had seen left as it
/// is, or at every write-back cut down to the events the state names and the frontier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WriteBack {
    Encoded,
    Uncompacted,
    Compacted,
}

impl WriteBack {
    /// Every way, the one the store takes first.
    pub(super) const EVERY: [WriteBack; 3] = [
        WriteBack::Encoded,
        WriteBack::Uncompacted,
        WriteBack::Compacted,
    ];

    /// Whether writing `projection` back cuts what every event had seen down to the events the
    /// state names and the frontier: as the store writes it, once the events taken in since it was
    /// last compacted are more than an eighth of those it knew then.
    fn compacts(self, projection: &Projection) -> bool {
        let since = projection.known() - projection.compacted();
        match self {
            WriteBack::Encoded => since * 8 > projection.compacted(),
            WriteBack::Compacted => true,
            WriteBack::Uncompacted => false,
        }
    }
}

/// The sum of bytes in slices is the sum of the bytes together, wherever they are cut: what a part
/// written back in slices is read against (roll-lqjj.51).
#[test]
fn a_sum_of_slices_is_the_sum_of_their_bytes_together() {
    let bytes: Vec<u8> = (0..300u32).map(|n| (n * 37 % 251) as u8).collect();
    for cut in [0, 1, 7, 8, 9, 64, 150, 299, 300] {
        for other in [cut, cut / 2, 300] {
            let (low, high) = (cut.min(other), cut.max(other));
            let slices = [&bytes[..low], &bytes[low..high], &[][..], &bytes[high..]];
            assert_eq!(sum_of(&slices), sum(&bytes), "{low} {high}");
        }
    }
    assert_ne!(sum_of(&[&bytes[..299]]), sum(&bytes));
}

/// The projection read back from its bytes, as the next command reads it, written back as `back`
/// says, its new texts appended to `file` and every issue's texts there. Read back with some
/// issues' texts read in alone, as a command that prints those issues whole reads it, every issue
/// answers who holds it as it did, and those read in their ownership history too
/// (`owned::read_alike`). What the write-back copied of the bytes the projection was read back
/// from is what it writes when it copies nothing, byte for byte (roll-lqjj.44).
fn read_back(projection: &Projection, file: &mut TextFile, back: WriteBack) -> Projection {
    let compacts = back.compacts(projection);
    let (encoded, _) = match back {
        WriteBack::Encoded => projection.encode(file.len() as u64),
        _ => projection.encode_with(file.len() as u64, compacts),
    };
    let (written, _) = projection.encode_copying(file.len() as u64, compacts, Copying::Nothing);
    if back == WriteBack::Encoded {
        // as the store writes it: in slices, summed as they are (roll-lqjj.51)
        let (parts, _) = projection.encode_in_parts(file.len() as u64);
        for (part, bytes) in [
            (&parts.state, &encoded.state),
            (&parts.index, &encoded.index),
        ] {
            let slices = part.slices();
            assert!(slices.concat() == *bytes && sum_of(&slices) == sum(bytes));
            assert!(
                slices.len() <= 1 + 2 * bytes.len().div_ceil(4096),
                "{}",
                slices.len()
            );
        }
    }
    for (part, copied, written) in [
        ("state", &encoded.state, &written.state),
        ("index", &encoded.index, &written.index),
    ] {
        let differs = (copied.iter().zip(written.iter())).position(|(a, b)| a != b);
        assert!(
            copied == written,
            "the {part} part copied differs from the one written, at byte {differs:?} of {}",
            copied.len()
        );
    }
    file.extend_from_slice(&encoded.texts);
    let decoded = Projection::decode(encoded.clone()).unwrap();
    assert!(decoded.state().issues().all(|issue| !issue.texts().here()));
    // each name and each value once in each table, however many write-backs added to it
    let kept = decoded.kept.as_deref().unwrap();
    for table in [&kept.state_table, &kept.index_table] {
        let once = |at: usize| !table.names[..at].contains(&table.names[at]);
        assert!((0..table.names.len()).all(once), "{:?}", table.names);
        let values: Vec<Vec<u8>> = table
            .values
            .iter()
            .map(|v| super::codec::bytes_of(v))
            .collect();
        let once = |at: usize| !values[..at].contains(&values[at]);
        assert!((0..values.len()).all(once), "{:?}", table.values);
    }
    let original = whole(projection, file);
    let read = whole(&decoded, file);
    assert!(same(read.state(), original.state()));
    assert_eq!(
        edges::metadata(read.state()),
        edges::metadata(original.state())
    );
    assert!(known::knows_alike(&decoded, projection, compacts));
    let mut state = decode_state(&encoded.state).unwrap();
    assert!(known::knows_writers_alone(&state, projection));
    assert_eq!(edges::outline(&state), edges::outline(original.state()));
    // as a command that prints some issues whole reads in their texts alone
    let some: Vec<EventId> = state.issues.keys().copied().step_by(2).collect();
    let mut partly = state.clone();
    read_in(&mut partly, file, Some(some.clone()));
    owned::read_alike(&partly, original.state(), &some);
    read_in(&mut state, file, None);
    assert!(same(&state, original.state()));
    assert_eq!(edges::metadata(&state), edges::metadata(original.state()));
    decoded
}

/// Whether `extended` is what projecting the whole journal (`full`) makes, and answers as it does
/// of every event its state names.
fn equal(extended: &Projection, full: &Projection) -> bool {
    same(extended.state(), full.state())
        && answers_alike(extended, full.state())
        && extended.heads == full.heads
        && extended.ids == full.ids
        && extended.lengths == full.lengths
        && extended.lasts == full.lasts
        && extended.open == full.open
}

/// Whether `projection` answers as `state`, the same journal's, of every event its state names and
/// its frontier: who wrote each, and whether each head's writer had seen it, what an event applied
/// after all of them asks (roll-lqjj.43).
fn answers_alike(projection: &Projection, state: &State) -> bool {
    let mut named = super::named(projection.state());
    named.extend(projection.heads.iter().copied());
    (named.iter()).all(|id| {
        projection.state().who(*id) == state.who(*id)
            && (projection.heads.iter())
                .all(|head| projection.state().saw(*head, *id) == state.saw(*head, *id))
    })
}

#[test]
fn every_extension_is_replays_answer_or_declined() {
    let tally = differential(0..40, false);
    // A local write builds on the frontier: the projection takes it, a takeover's excepted. A
    // merge it takes when the merged events build on it.
    assert!(
        tally.declined.is_empty(),
        "local writes declined in seeds {:?}",
        tally.declined
    );
    assert!(tally.merged > 50, "{} merges taken in", tally.merged);
    assert_eq!(tally.closed, 0);
    // a projection written back several batches before takes them all in at once (roll-lqjj.36)
    assert!(
        tally.deferred > 500 && tally.longest >= 8,
        "{} extensions by several batches, {} at most",
        tally.deferred,
        tally.longest
    );
    // written back as it was, what every event had seen holds events the state no longer names
    // (roll-lqjj.43)
    assert!(
        tally.uncompacted > 500 && tally.unnamed >= 20,
        "{} write-backs left events the state no longer names, {} at most",
        tally.uncompacted,
        tally.unnamed
    );
    // and as the store writes it back, on both sides of an eighth more
    assert!(
        tally.left > 200 && tally.cut > 200,
        "the store's write-backs: {} left such events, {} compacted",
        tally.left,
        tally.cut
    );
    edges::pinned("plain", &tally.answers);
    tally.drawn.enough();
    // every kind of ownership record, a takeover's reason, a lost claim and an end too late among
    // them, and later events adding to histories a projection read back holds
    assert!(tally.seen.more_than(5), "{:?}", tally.seen);
    assert_eq!(tally.owned, owned::DIFFERENTIAL, "{:#x}", tally.owned);
    tally.marked.enough();
    // the store's own writes, extended as their transaction holds them (roll-lqjj.51)
    assert!(tally.written > 800, "{} writes", tally.written);
}
