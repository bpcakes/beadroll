//! Machines writing one tracker, as `Tx.emit` writes: each event names the heads of what its machine
//! has seen, machines sync now and then, claims may land in attempt files, and a copied identity
//! breaks its journal's chain.

use std::collections::BTreeSet;

use beadroll_core::json::{Int, Object, Value};

use super::Tape;
use super::hostile;
use crate::diff::cases::Case;
use crate::diff::report::Op;

pub(super) const TEXTS: [&str; 13] = [
    "",
    "fix the parser",
    "Ünïcödé ✓ ⚠",
    "tab\there",
    "line\nbreak",
    "quote ' \" \\",
    "\u{1b}[31mred\u{1b}[0m",
    "😀 emoji",
    "  spaced  ",
    "\u{2028}sep\u{85}",
    "\u{fffd}",
    "nul\u{0}byte",
    "x",
];
const LABELS: [&str; 8] = [
    "bug",
    "p1",
    "rust-port",
    "Bug",
    "ünï",
    "",
    "a,b",
    " spaced ",
];
const TYPES: [&str; 6] = ["task", "bug", "epic", "feature", "chore", "epic"];
const STATUSES: [&str; 8] = [
    "open",
    "in_progress",
    "blocked",
    "deferred",
    "closed",
    "tombstone",
    "review",
    "custom-1",
];
const DEP_TYPES: [Option<&str>; 6] = [
    Some("blocks"),
    Some("parent-child"),
    Some("related"),
    Some("blocks"),
    None,
    Some(""),
];

/// One machine: its journal, its attempt files, and the events it has seen.
struct Machine {
    name: String,
    journal: Vec<usize>,
    attempts: Vec<Vec<usize>>,
    known: BTreeSet<usize>,
}

/// The tracker being written.
pub(super) struct World<'t> {
    pub(super) tape: &'t mut Tape,
    /// How hostile this case is: 0 writes only what bead writes, 1 a little damage, 4 a lot.
    level: u64,
    millis: u64,
    machines: Vec<Machine>,
    /// Every event made, with the events it names.
    pub(super) events: Vec<(Object, Vec<usize>)>,
    /// Issues: uid and key.
    issues: Vec<(String, String)>,
    /// Claims: event id and issue uid.
    claims: Vec<(String, String)>,
}

fn text(value: &str) -> Value {
    Value::from(value)
}

fn obj(members: Vec<(&str, Value)>) -> Object {
    members
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
}

impl World<'_> {
    /// True `percent` times in a hundred, scaled by the case's hostility (`percent` is the heavy
    /// rate).
    pub(super) fn hostile(&mut self, percent: u64) -> bool {
        let scaled = (percent * self.level).min(400);
        self.level > 0 && self.tape.word() % 400 >= 400 - scaled
    }

    pub(super) fn id(&self, at: usize) -> String {
        self.events
            .get(at)
            .and_then(|(e, _)| e.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    /// A UUIDv7 at the current time, its random bits from the tape.
    pub(super) fn new_id(&mut self) -> String {
        let ms = self.millis;
        let (r12, r14, r48) = (
            self.tape.word() & 0xfff,
            self.tape.word() & 0x3fff,
            self.tape.word() & 0xffff_ffff_ffff,
        );
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            ms >> 16,
            ms & 0xffff,
            0x7000 | r12,
            0x8000 | r14,
            r48
        )
    }

    /// The events machine `m` has seen that nothing it has seen builds on.
    fn heads(&self, m: usize) -> Vec<usize> {
        let Some(machine) = self.machines.get(m) else {
            return Vec::new();
        };
        let referenced: BTreeSet<usize> = machine
            .known
            .iter()
            .filter_map(|e| self.events.get(*e))
            .flat_map(|(_, deps)| deps.iter().copied())
            .collect();
        let mut heads: Vec<usize> = machine
            .known
            .iter()
            .copied()
            .filter(|e| !referenced.contains(e))
            .collect();
        heads.sort_by_key(|e| self.id(*e));
        heads
    }

    fn actor(&mut self, m: usize) -> Option<Value> {
        let thread = format!("t{m}");
        let actors = [
            obj(vec![("agent", text("claude")), ("thread", text(&thread))]),
            obj(vec![("agent", text("codex")), ("thread", text("t9"))]),
            obj(vec![("name", text("Ann"))]),
            Object::new(),
            obj(vec![("agent", Value::Null), ("thread", text(&thread))]),
            obj(vec![("agent", text("claude")), ("thread", text(""))]),
            obj(vec![
                ("agent", text("claude")),
                ("thread", text(&thread)),
                ("machine", text("spoof")),
                ("x", Value::from(1)),
            ]),
        ];
        let at = if self.tape.chance(70) {
            0
        } else {
            self.tape.below(actors.len())
        };
        let actor = actors.get(at).cloned()?;
        if self.tape.chance(3) {
            None
        } else {
            Some(Value::Object(actor))
        }
    }

    fn place(&mut self) -> Option<Value> {
        if !self.tape.chance(40) {
            return None;
        }
        let places = [
            obj(vec![
                ("branch", text("main")),
                ("commit", text("0123456789abcdef")),
            ]),
            obj(vec![("branch", Value::Null), ("commit", text("fedcba"))]),
            Object::new(),
            obj(vec![("branch", text("feature/ü"))]),
        ];
        let at = self.tape.below(places.len());
        places.get(at).cloned().map(Value::Object)
    }

    /// Writes an event on machine `m`, as `Tx.emit` does; `None` for the issue names the event itself.
    pub(super) fn emit(
        &mut self,
        m: usize,
        op: &str,
        issue: Option<String>,
        data: Object,
        attempt: bool,
    ) -> usize {
        self.millis += u64::try_from(self.tape.below(3)).unwrap_or(0);
        let id = self.new_id();
        let mut deps = self.heads(m);
        if self.hostile(2) && !deps.is_empty() {
            // A copied identity: built on something older than its own last event.
            let at = self.tape.below(deps.len());
            deps.remove(at);
            if let Some(&older) = self
                .machines
                .get(m)
                .and_then(|machine| machine.journal.first())
            {
                deps.push(older);
            }
        }
        let replica = self
            .machines
            .get(m)
            .map(|machine| machine.name.clone())
            .unwrap_or_default();
        let mut event = Object::new();
        event.insert("v".into(), Value::from(1));
        event.insert("id".into(), text(&id));
        event.insert("replica".into(), text(&replica));
        event.insert(
            "deps".into(),
            Value::Array(deps.iter().map(|d| text(&self.id(*d))).collect()),
        );
        if let Some(actor) = self.actor(m) {
            event.insert("actor".into(), actor);
        }
        event.insert("op".into(), text(op));
        event.insert("issue".into(), text(&issue.unwrap_or_else(|| id.clone())));
        event.insert("data".into(), Value::Object(data));
        if let Some(place) = self.place() {
            event.insert("where".into(), place);
        }
        let at = self.events.len();
        self.events.push((event, deps));
        if let Some(machine) = self.machines.get_mut(m) {
            machine.known.insert(at);
            if attempt {
                match machine.attempts.last_mut() {
                    Some(last) if last.len() < 3 && self.tape.chance(50) => last.push(at),
                    _ => machine.attempts.push(vec![at]),
                }
            } else {
                machine.journal.push(at);
            }
        }
        at
    }

    /// Machine `m` fetches what another machine (or every machine) has.
    fn sync(&mut self, m: usize) {
        let everyone = self.tape.chance(50);
        let other = self.tape.below(self.machines.len());
        let learned: Vec<usize> = self
            .machines
            .iter()
            .enumerate()
            .filter(|(n, _)| everyone || *n == other)
            .flat_map(|(_, machine)| machine.known.iter().copied())
            .collect();
        if let Some(machine) = self.machines.get_mut(m) {
            machine.known.extend(learned);
        }
    }

    fn issue(&mut self) -> Option<(String, String)> {
        if self.tape.chance(5) {
            return Some((self.new_id(), "ex-gone".into())); // an issue nobody created
        }
        let issues = self.issues.clone();
        self.tape.pick(&issues).cloned()
    }

    fn text(&mut self) -> Value {
        let at = self.tape.below(TEXTS.len());
        text(TEXTS.get(at).copied().unwrap_or_default())
    }

    fn labels(&mut self) -> Value {
        let count = self.tape.below(4);
        Value::Array(
            (0..count)
                .map(|_| {
                    text(
                        LABELS
                            .get(self.tape.below(LABELS.len()))
                            .copied()
                            .unwrap_or_default(),
                    )
                })
                .collect(),
        )
    }

    pub(super) fn number(&mut self) -> Value {
        let numbers = [
            Value::from(0),
            Value::from(1),
            Value::from(2),
            Value::from(3),
            Value::from(4),
            Value::Bool(true),
            Value::Bool(false),
            Value::from(30),
            Value::from(-5),
            Value::from(2_147_483_647),
            Value::from(i64::MAX),
            Value::Int(
                Int::parse("123456789012345678901234567890").unwrap_or_else(|| Int::from(0)),
            ),
        ];
        let at = if self.tape.chance(60) {
            self.tape.below(5)
        } else {
            self.tape.below(numbers.len())
        };
        numbers.get(at).cloned().unwrap_or(Value::Null)
    }

    fn key(&mut self, parent: Option<&str>) -> String {
        if let Some(parent) = parent {
            let children = self
                .issues
                .iter()
                .filter(|(_, k)| k.starts_with(parent))
                .count();
            return format!("{parent}.{children}");
        }
        if !self.issues.is_empty() && self.tape.chance(15) {
            let keys: Vec<String> = self.issues.iter().map(|(_, k)| k.clone()).collect();
            return self.tape.pick(&keys).cloned().unwrap_or_default(); // minted twice before a sync
        }
        const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let size = if self.tape.chance(80) { 1 } else { 4 };
        let suffix: String = (0..size)
            .map(|_| char::from(DIGITS.get(self.tape.below(36)).copied().unwrap_or(b'0')))
            .collect();
        if self.tape.chance(10) {
            format!("x.{}", self.tape.below(3) + 7)
        } else {
            format!("ex-{suffix}")
        }
    }

    fn fields(&mut self, creating: bool) -> Object {
        let mut fields = Object::new();
        let names = [
            "title",
            "priority",
            "issue_type",
            "status",
            "description",
            "assignee",
            "estimated_minutes",
            "external_ref",
            "notes",
            "design",
            "acceptance_criteria",
            "closed_at",
            "close_reason",
        ];
        let count = 1 + self.tape.below(if creating { 5 } else { 3 });
        for _ in 0..count {
            let name = names
                .get(self.tape.below(names.len()))
                .copied()
                .unwrap_or("title");
            let value = match name {
                "priority" | "estimated_minutes" => self.number(),
                "issue_type" => text(
                    TYPES
                        .get(self.tape.below(TYPES.len()))
                        .copied()
                        .unwrap_or("task"),
                ),
                "status" => text(
                    STATUSES
                        .get(self.tape.below(STATUSES.len()))
                        .copied()
                        .unwrap_or("open"),
                ),
                "assignee" if self.tape.chance(30) => Value::Null,
                "closed_at" => text("2026-01-02T03:04:05Z"),
                _ => self.text(),
            };
            fields.insert(name.into(), value);
        }
        fields
    }

    fn create(&mut self, m: usize) {
        let parent = if self.tape.chance(25) {
            self.issues.last().cloned()
        } else {
            None
        };
        let key = self.key(parent.as_ref().map(|(_, k)| k.as_str()));
        let mut data = obj(vec![
            ("key", text(&key)),
            ("fields", Value::Object(self.fields(true))),
        ]);
        if self.tape.chance(30) {
            data.insert("labels".into(), self.labels());
        }
        if self.tape.chance(15) {
            self.imported(&mut data);
        }
        let at = self.emit(m, "create", None, data, false);
        let uid = self.id(at);
        self.issues.push((uid.clone(), key));
        if let Some((parent, _)) = parent {
            let data = obj(vec![("to", text(&parent)), ("type", text("parent-child"))]);
            self.emit(m, "dep_add", Some(uid), data, false);
        }
    }

    /// What an import adds to a create: comments, extras, dependencies on keys, bookkeeping fields.
    fn imported(&mut self, data: &mut Object) {
        data.insert("imported".into(), Value::Bool(true));
        let comment = obj(vec![
            (
                "legacy_id",
                if self.tape.chance(50) {
                    Value::from(7)
                } else {
                    Value::Null
                },
            ),
            ("author", text("bob")),
            ("text", self.text()),
            ("created_at", Value::Null),
        ]);
        data.insert(
            "comments".into(),
            Value::Array(vec![Value::Object(comment)]),
        );
        let extra = obj(vec![
            ("source_repo", text(".")),
            ("score", Value::Float(0.5)),
            ("status", text("forged")),
        ]);
        data.insert("extra".into(), Value::Object(extra));
        let target = if self.tape.chance(50) {
            "ex-late".to_owned()
        } else {
            self.key(None)
        };
        let kind = DEP_TYPES
            .get(self.tape.below(DEP_TYPES.len()))
            .copied()
            .flatten();
        let dangling = obj(vec![
            ("issue_id", text("ex-old")),
            ("depends_on_id", text(&target)),
            ("type", kind.map_or(Value::Null, text)),
            ("created_at", text("2025-01-01T00:00:00Z")),
            ("metadata", text("{\"k\":1}")),
        ]);
        data.insert(
            "dangling".into(),
            Value::Array(vec![Value::Object(dangling)]),
        );
        if let Some(Value::Object(fields)) = data.get_mut("fields") {
            fields.insert("created_at".into(), text("2025-06-01T00:00:00Z"));
            if self.tape.chance(50) {
                fields.insert("created_by".into(), text("importer"));
            }
        }
    }

    fn claim(&mut self, m: usize, uid: String) {
        let mut data = Object::new();
        let current: Vec<String> = self
            .claims
            .iter()
            .filter(|(_, u)| *u == uid)
            .map(|(c, _)| c.clone())
            .collect();
        match self.tape.below(4) {
            0 => {
                data.insert("mode".into(), text("offline"));
            }
            1 => {
                if let Some(previous) = self.tape.pick(&current).cloned() {
                    data.insert("takeover".into(), text(&previous));
                    data.insert("reason".into(), text("same task, new session"));
                }
            }
            _ => {}
        }
        if self.tape.chance(30) {
            data.insert("generation".into(), self.number());
        }
        let attempt = self.tape.chance(50);
        let at = self.emit(m, "claim", Some(uid.clone()), data, attempt);
        self.claims.push((self.id(at), uid.clone()));
        if attempt && self.tape.chance(30) {
            let data = obj(vec![(
                "fields",
                Value::Object(obj(vec![("status", text("in_progress"))])),
            )]);
            self.emit(m, "set", Some(uid), data, true); // an edit made with the claim lands with it
        }
    }

    fn end(&mut self, m: usize, op: &str, uid: String) {
        let mut data = Object::new();
        let current: Vec<String> = self
            .claims
            .iter()
            .filter(|(_, u)| *u == uid)
            .map(|(c, _)| c.clone())
            .collect();
        if let Some(claim) = self
            .tape
            .pick(&current)
            .cloned()
            .filter(|_| self.tape.chance(70))
        {
            data.insert("claim".into(), text(&claim));
        }
        if op != "release" || self.tape.chance(30) {
            data.insert("reason".into(), self.text());
        }
        let attempt = op == "release" && self.tape.chance(50);
        self.emit(m, op, Some(uid), data, attempt);
    }

    fn dependency(&mut self, m: usize, uid: String, adding: bool) {
        let target = match self.tape.below(5) {
            0 => self
                .issues
                .get(self.tape.below(self.issues.len()))
                .map(|(_, k)| k.clone())
                .unwrap_or_default(),
            1 => "ex-late".into(),
            _ => self
                .issues
                .get(self.tape.below(self.issues.len()))
                .map(|(u, _)| u.clone())
                .unwrap_or_default(),
        };
        let kind = DEP_TYPES
            .get(self.tape.below(DEP_TYPES.len()))
            .copied()
            .flatten();
        let mut data = Object::new();
        if !adding && self.tape.chance(30) {
            data.insert(
                "key".into(),
                text(if self.tape.chance(50) {
                    "ex-late"
                } else {
                    &target
                }),
            );
        } else {
            data.insert("to".into(), text(&target));
        }
        if adding || self.tape.chance(50) {
            data.insert("type".into(), kind.map_or(Value::Null, text));
        }
        if adding && self.tape.chance(20) {
            data.insert(
                "meta".into(),
                Value::Object(obj(vec![
                    ("thread_id", text("t")),
                    ("created_at", Value::from(5)),
                ])),
            );
        }
        self.emit(
            m,
            if adding { "dep_add" } else { "dep_remove" },
            Some(uid),
            data,
            false,
        );
    }

    fn step(&mut self) {
        let m = self.tape.below(self.machines.len());
        if self.tape.chance(15) {
            self.sync(m);
            return;
        }
        let roll = self.tape.below(100);
        if self.issues.is_empty() || roll < 22 {
            self.create(m);
            return;
        }
        let Some((uid, _)) = self.issue() else { return };
        match roll {
            22..=36 => {
                let data = obj(vec![("fields", Value::Object(self.fields(false)))]);
                self.emit(m, "set", Some(uid), data, false);
            }
            37..=48 => self.claim(m, uid),
            49..=54 => self.end(m, "release", uid),
            55..=62 => self.end(m, "close", uid),
            63..=66 => {
                self.emit(m, "reopen", Some(uid), Object::new(), false);
            }
            67..=68 => self.end(m, "delete", uid),
            69..=76 => {
                let mut data = obj(vec![("text", self.text())]);
                if self.tape.chance(20) {
                    data.insert("author".into(), self.text());
                }
                self.emit(m, "comment", Some(uid), data, false);
            }
            77..=85 => self.dependency(m, uid, true),
            86..=90 => self.dependency(m, uid, false),
            _ => {
                let ops = ["label_add", "label_remove", "label_set"];
                let op = ops.get(self.tape.below(3)).copied().unwrap_or("label_add");
                let data = obj(vec![("labels", self.labels())]);
                self.emit(m, op, Some(uid), data, false);
            }
        }
    }
}

/// One generated case.
pub(super) fn history(tape: &mut Tape, name: &str) -> Case {
    let count = 1 + tape.below(3);
    let machines = (0..count)
        .map(|n| Machine {
            name: format!(
                "{n:04x}{}",
                ["aa", "bb", "cc"].get(n).copied().unwrap_or("dd")
            ),
            journal: Vec::new(),
            attempts: Vec::new(),
            known: BTreeSet::new(),
        })
        .collect();
    let level = [0, 0, 1, 1, 4].get(tape.below(5)).copied().unwrap_or(0);
    let mut world = World {
        tape,
        level,
        millis: 1_791_558_245_123,
        machines,
        events: Vec::new(),
        issues: Vec::new(),
        claims: Vec::new(),
    };
    if !world.tape.chance(5) {
        let prefix = if world.tape.chance(5) {
            world.number()
        } else {
            text("ex")
        };
        world.emit(
            0,
            "init",
            None,
            obj(vec![("prefix", prefix), ("name", text("ex"))]),
            false,
        );
        if world.tape.chance(5) && count > 1 {
            world.emit(
                1,
                "init",
                None,
                obj(vec![("prefix", text("zz")), ("name", text("zz"))]),
                false,
            );
        }
    }
    let steps = world.tape.below(60);
    for _ in 0..steps {
        world.step();
    }
    let ops = hostile::encode(&mut world);
    Case {
        name: name.to_owned(),
        ops,
    }
}

impl World<'_> {
    /// Each machine's journal and attempt files, as event objects, in order.
    pub(super) fn files(&self) -> Vec<(String, Vec<Object>, Vec<Vec<Object>>)> {
        let event = |at: &usize| {
            self.events
                .get(*at)
                .map(|(e, _)| e.clone())
                .unwrap_or_default()
        };
        self.machines
            .iter()
            .map(|machine| {
                let journal = machine.journal.iter().map(event).collect();
                let attempts = machine
                    .attempts
                    .iter()
                    .map(|a| a.iter().map(event).collect())
                    .collect();
                (machine.name.clone(), journal, attempts)
            })
            .collect()
    }

    /// The keys of the issues made so far.
    pub(super) fn known_keys(&self) -> Vec<String> {
        self.issues.iter().map(|(_, key)| key.clone()).collect()
    }

    /// Adds `Op`s for raw values: what the journal must survive whatever a test hands it.
    pub(super) fn raw_ops(&mut self, ops: &mut Vec<Op>) {
        let samples = [
            Value::from(5),
            Value::from("text"),
            Value::Array(vec![]),
            Value::Object(obj(vec![("id", text("nope"))])),
            Value::Object(obj(vec![("id", Value::from(3))])),
            Value::Null,
        ];
        for _ in 0..self.tape.below(3) {
            let value = samples
                .get(self.tape.below(samples.len()))
                .cloned()
                .unwrap_or(Value::Null);
            ops.push(Op::Add {
                event: value,
                replica: "0000aa".into(),
                origin: "raw".into(),
                sequenced: true,
            });
        }
    }
}
