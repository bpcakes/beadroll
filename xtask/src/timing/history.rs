//! The histories `timing --replay` measures: six machines writing one tracker, every event valid,
//! each machine building on what it has seen and syncing now and then. Each comes as the files a
//! store holds: every machine's journal segment, and its attempt files (the ownership events a
//! coordinated command published, which have no place in the journal's order).

use std::collections::BTreeSet;

use beadroll_core::json::{Format, Object, Value, write};

/// Which history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    /// Creates, edits, comments, labels, dependencies, claims and closes, every event in its
    /// machine's journal: the history `timing --replay` has always measured.
    Plain,
    /// Heavy in ownership: about half the events are claims, takeovers and releases, each in an
    /// attempt file of its own as a coordinated command publishes it (with the edits an
    /// `update --claim` makes beside it), decided against everything the machines have published;
    /// owners edit, comment and close naming their claim in between.
    Ownership,
    /// Heavy in churn: [`CHURN_ISSUES`] issues, then only titles changed and a label put on and
    /// taken off, so that the state stays the size it is whatever the number of events: what the
    /// projection's cost follows (roll-lqjj.2).
    Churn,
    /// Heavy in text, as real trackers are (roll-lqjj.6): the plain history's operations, but a
    /// create describes its issue in 1.2-2.4 KB, an edit rewrites the description or the notes in
    /// 0.6-1.8 KB and a comment says 0.3-1.5 KB, so that texts are most of the state.
    Text,
}

impl Shape {
    /// How a record names it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Shape::Plain => "plain",
            Shape::Ownership => "ownership",
            Shape::Churn => "churn",
            Shape::Text => "text",
        }
    }

    /// Its events, as a label names them.
    pub(crate) fn events(self) -> &'static str {
        match self {
            Shape::Plain => "events",
            Shape::Ownership => "ownership-heavy events",
            Shape::Churn => "events on a thousand issues",
            Shape::Text => "text-heavy events",
        }
    }
}

/// How many issues the churn history has.
pub(crate) const CHURN_ISSUES: usize = 1000;

/// One machine's files, as a store holds them.
pub(crate) struct Files {
    /// The machine.
    pub(crate) replica: String,
    /// Its journal: one segment.
    pub(crate) segment: Vec<u8>,
    /// Its attempt files, each named after its first event, in the order written.
    pub(crate) attempts: Vec<(String, Vec<u8>)>,
}

/// The rotation size bead uses when `BEAD_ROTATE_BYTES` is unset (`beadroll_store::ROTATE_BYTES`).
pub(crate) const ROTATE_BYTES: usize = 256 * 1024;

/// A machine's journal cut into segments as bead rotates it: a new one started once the last has
/// reached `rotate` bytes, each of whole lines.
pub(crate) fn segments(journal: &[u8], rotate: usize) -> Vec<&[u8]> {
    let mut cuts: Vec<&[u8]> = Vec::new();
    let mut start = 0;
    for (at, byte) in journal.iter().enumerate() {
        if *byte == b'\n' && at + 1 - start >= rotate {
            cuts.push(&journal[start..=at]);
            start = at + 1;
        }
    }
    if start < journal.len() {
        cuts.push(&journal[start..]);
    }
    cuts
}

/// A history of `count` events.
pub(crate) fn history(count: usize, shape: Shape) -> Vec<Files> {
    let mut writer = Writer::new();
    match shape {
        Shape::Plain => plain(&mut writer, count, Shape::Plain),
        Shape::Ownership => ownership(&mut writer, count),
        Shape::Churn => churn(&mut writer, count),
        Shape::Text => plain(&mut writer, count, Shape::Text),
    }
    writer.files
}

/// The id of a history's event `n`, counted from its first: a UUIDv7 one millisecond after event
/// `n - 1`'s.
pub(crate) fn event_id(n: usize) -> String {
    let millis = 1_791_558_245_123 + u64::try_from(n).unwrap_or(0);
    format!(
        "{:08x}-{:04x}-7000-8000-{:012x}",
        millis >> 16,
        millis & 0xffff,
        n
    )
}

/// Six machines' files, and what each has seen.
struct Writer {
    seed: u64,
    files: Vec<Files>,
    /// What each machine's next event names as its causes.
    heads: Vec<Vec<String>>,
    /// Events written so far.
    written: usize,
}

impl Writer {
    fn new() -> Writer {
        let files: Vec<Files> = (0..6)
            .map(|n| Files {
                replica: format!("{n:04x}machine"),
                segment: Vec::new(),
                attempts: Vec::new(),
            })
            .collect();
        Writer {
            seed: 0x5EED,
            heads: vec![Vec::new(); files.len()],
            files,
            written: 0,
        }
    }

    /// A number below `n`, from a fixed sequence: every run draws the same history.
    fn next(&mut self, n: usize) -> usize {
        self.seed = self
            .seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from((self.seed >> 33) % u64::try_from(n.max(1)).unwrap_or(1)).unwrap_or(0)
    }

    /// The id of the next event: a UUIDv7 one millisecond after the last.
    fn id(&self) -> String {
        event_id(self.written)
    }

    /// Machine `m` learns what machine `other` has seen.
    fn sync(&mut self, m: usize, other: usize) {
        let union: Vec<String> = self.heads[m]
            .iter()
            .chain(&self.heads[other])
            .cloned()
            .collect();
        self.heads[m] = union;
    }

    /// Machine `m` fetches everything the machines have published, as a coordinated command
    /// does before it decides anything.
    fn fetch(&mut self, m: usize) {
        let all: BTreeSet<String> = self.heads.iter().flatten().cloned().collect();
        self.heads[m] = all.into_iter().collect();
    }

    /// Event `id` by machine `m`, as `Tx.emit` writes it, naming what `m` has seen.
    fn event(&mut self, m: usize, id: &str, op: &str, issue: &str, data: Value) -> Vec<u8> {
        let deps: Vec<Value> = self.heads[m]
            .iter()
            .map(|d| Value::from(d.as_str()))
            .collect();
        let actor = obj(&[
            ("agent", "claude".into()),
            ("thread", format!("t{m}").into()),
        ]);
        let event = obj(&[
            ("v", Value::from(1)),
            ("id", id.into()),
            ("replica", self.files[m].replica.clone().into()),
            ("deps", Value::Array(deps)),
            ("actor", actor),
            ("op", op.into()),
            ("issue", issue.into()),
            ("data", data),
        ]);
        self.heads[m] = vec![id.to_owned()];
        self.written += 1;
        let mut line = write(&event, Format::COMPACT)
            .unwrap_or_default()
            .into_bytes();
        line.push(b'\n');
        line
    }

    /// Appends an event to machine `m`'s journal; `None` for the issue names the event itself.
    /// Returns its id.
    fn append(&mut self, m: usize, op: &str, issue: Option<&str>, data: Value) -> String {
        let id = self.id();
        let line = self.event(m, &id, op, issue.unwrap_or(&id), data);
        self.files[m].segment.extend(line);
        id
    }

    /// Writes one attempt file on machine `m`, its events building on each other. Returns their ids.
    fn attempt(&mut self, m: usize, issue: &str, events: Vec<(&str, Value)>) -> Vec<String> {
        let (mut ids, mut bytes) = (Vec::new(), Vec::new());
        for (op, data) in events {
            let id = self.id();
            bytes.extend(self.event(m, &id, op, issue, data));
            ids.push(id);
        }
        let name = format!("{}.json", ids.first().map_or("", String::as_str));
        self.files[m].attempts.push((name, bytes));
        ids
    }
}

/// The history `timing --replay` has always measured, draw for draw; with `texts`, the same
/// operations carrying prose (`Shape::Text`), which takes draws of its own.
fn plain(w: &mut Writer, count: usize, texts: Shape) {
    let texts = matches!(texts, Shape::Text);
    let mut issues: Vec<String> = Vec::new();
    let mut claims: Vec<(String, String)> = Vec::new();
    for n in 0..count {
        let m = if n == 0 { 0 } else { w.next(6) };
        if n > 0 && w.next(10) == 0 {
            let other = w.next(6);
            w.sync(m, other);
        }
        if n == 0 {
            w.append(m, "init", None, init());
            continue;
        }
        if issues.is_empty() || w.next(10) == 0 {
            let mut data = create(n);
            if texts {
                let description = prose(w, 1200);
                described(&mut data, "description", description);
            }
            issues.push(w.append(m, "create", None, data));
            continue;
        }
        let issue = issues[w.next(issues.len())].clone();
        let (op, data) = match w.next(10) {
            0..=2 if texts => {
                let field = if w.next(2) == 0 {
                    "description"
                } else {
                    "notes"
                };
                let mut data = edit(n);
                described(&mut data, field, prose(w, 600));
                ("set", data)
            }
            0..=2 => ("set", edit(n)),
            3 | 4 if texts => ("comment", obj(&[("text", prose(w, 300).into())])),
            3 | 4 => ("comment", obj(&[("text", format!("note {n}").into())])),
            5 => (
                "label_add",
                obj(&[("labels", Value::Array(vec!["p1".into()]))]),
            ),
            6 => {
                let to = issues[w.next(issues.len())].clone();
                (
                    "dep_add",
                    obj(&[("to", to.into()), ("type", "blocks".into())]),
                )
            }
            7 => {
                claims.push((w.id(), issue.clone()));
                ("claim", Value::from(Object::new()))
            }
            8 => match claims.iter().rev().find(|(_, on)| *on == issue) {
                Some((claim, _)) => ("release", obj(&[("claim", claim.clone().into())])),
                None => ("reopen", Value::from(Object::new())),
            },
            _ => ("close", obj(&[("reason", "done".into())])),
        };
        w.append(m, op, Some(&issue), data);
    }
}

/// A history heavy in churn (`Shape::Churn`).
fn churn(w: &mut Writer, count: usize) {
    let mut issues: Vec<String> = Vec::new();
    for n in 0..count {
        let m = if n == 0 { 0 } else { w.next(6) };
        if n > 0 && w.next(10) == 0 {
            let other = w.next(6);
            w.sync(m, other);
        }
        if n == 0 {
            w.append(m, "init", None, init());
            continue;
        }
        if issues.len() < CHURN_ISSUES {
            issues.push(w.append(m, "create", None, create(n)));
            continue;
        }
        let issue = issues[w.next(issues.len())].clone();
        let label = || obj(&[("labels", Value::Array(vec!["p1".into()]))]);
        let (op, data) = match w.next(3) {
            0 => ("set", edit(n)),
            1 => ("label_add", label()),
            _ => ("label_remove", label()),
        };
        w.append(m, op, Some(&issue), data);
    }
}

/// An issue's claim as the machines agree on it.
struct Held {
    claim: String,
    owner: usize,
}

/// A history heavy in ownership (`Shape::Ownership`).
fn ownership(w: &mut Writer, count: usize) {
    // Each issue, how many times it was claimed, and its claim, if it has one.
    let mut issues: Vec<(String, i64, Option<Held>)> = Vec::new();
    while w.written < count {
        let n = w.written;
        if n == 0 {
            w.append(0, "init", None, init());
            continue;
        }
        let m = w.next(6);
        if w.next(10) == 0 {
            let other = w.next(6);
            w.sync(m, other);
        }
        if issues.is_empty() || w.next(100) < 8 {
            let uid = w.append(m, "create", None, create(n));
            issues.push((uid, 0, None));
            continue;
        }
        let at = w.next(issues.len());
        let (roll, offline, with_edit) = (w.next(100), w.next(10) == 0, w.next(4) == 0);
        let Some((uid, generation, held)) = issues.get_mut(at) else {
            continue;
        };
        let uid = uid.clone();
        let owner = held.as_ref().map(|held| held.owner);
        match roll {
            0..=14 => {
                w.append(m, "set", Some(&uid), edit(n));
            }
            15..=24 => {
                let data = obj(&[("text", format!("note {n}").into())]);
                w.append(m, "comment", Some(&uid), data);
            }
            25..=29 => {
                let data = obj(&[("labels", Value::Array(vec!["p1".into()]))]);
                w.append(m, "label_add", Some(&uid), data);
            }
            30..=39 => {
                // `bead close` by the owner names the claim it ends; anyone else's ends it too.
                let mut data = vec![("reason", Value::from("done"))];
                if let Some(held) = held.take().filter(|held| held.owner == m) {
                    data.push(("claim", held.claim.into()));
                }
                w.append(m, "close", Some(&uid), obj(&data));
            }
            40..=44 => {
                w.append(m, "reopen", Some(&uid), Value::from(Object::new()));
            }
            _ if offline => {
                // Claimed offline: provisional, never displacing a confirmed owner.
                let data = obj(&[
                    ("mode", "offline".into()),
                    ("generation", Value::from(*generation + 1)),
                ]);
                w.attempt(m, &uid, vec![("claim", data)]);
            }
            _ if owner == Some(m) => {
                let claim = held.take().map(|held| held.claim).unwrap_or_default();
                w.fetch(m);
                w.attempt(m, &uid, vec![("release", obj(&[("claim", claim.into())]))]);
            }
            _ => {
                *generation += 1;
                let mut data = vec![
                    ("mode", Value::from("online")),
                    ("generation", Value::from(*generation)),
                ];
                if let Some(held) = held.as_ref() {
                    data.push(("takeover", held.claim.clone().into()));
                    data.push(("reason", "same task, new session".into()));
                }
                let mut events = vec![("claim", obj(&data))];
                if with_edit && w.written + 2 <= count {
                    events.push(("set", edit(n))); // `update --claim` with an edit: one attempt
                }
                w.fetch(m);
                let ids = w.attempt(m, &uid, events);
                let claim = ids.into_iter().next().unwrap_or_default();
                *held = Some(Held { claim, owner: m });
            }
        }
    }
}

fn init() -> Value {
    obj(&[("prefix", "ex".into()), ("name", "ex".into())])
}

fn create(n: usize) -> Value {
    let fields = obj(&[
        ("title", format!("issue {n}").into()),
        ("priority", Value::from(2)),
    ]);
    obj(&[("key", format!("ex-{n}").into()), ("fields", fields)])
}

fn edit(n: usize) -> Value {
    obj(&[("fields", obj(&[("title", format!("edit {n}").into())]))])
}

/// Prose of `at_least` bytes and up to twice that, in words drawn from a fixed vocabulary.
fn prose(w: &mut Writer, at_least: usize) -> String {
    const WORDS: [&str; 32] = [
        "the", "replay", "of", "a", "journal", "keeps", "every", "event", "and", "its", "causes",
        "so", "that", "each", "machine", "agrees", "on", "what", "an", "issue", "says", "when",
        "work", "merges", "after", "days", "offline", "with", "tests", "that", "pin", "it",
    ];
    let target = at_least + w.next(at_least + 1);
    let mut text = String::with_capacity(target + 16);
    while text.len() < target {
        if !text.is_empty() {
            text.push(if w.next(12) == 0 { '\n' } else { ' ' });
        }
        text.push_str(WORDS[w.next(WORDS.len())]);
    }
    text
}

/// Sets `field` to `text` among the fields of a create's or an edit's `data`.
fn described(data: &mut Value, field: &str, text: String) {
    if let Value::Object(members) = data
        && let Some(Value::Object(fields)) = members.get_mut("fields")
    {
        fields.insert(field.to_owned(), Value::from(text));
    }
}

fn obj(members: &[(&str, Value)]) -> Value {
    let members: Object = members
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    Value::from(members)
}

#[cfg(test)]
mod tests;
