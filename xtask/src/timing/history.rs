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
}

impl Shape {
    /// How a record names it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Shape::Plain => "plain",
            Shape::Ownership => "ownership",
            Shape::Churn => "churn",
        }
    }

    /// Its events, as a label names them.
    pub(crate) fn events(self) -> &'static str {
        match self {
            Shape::Plain => "events",
            Shape::Ownership => "ownership-heavy events",
            Shape::Churn => "events on a thousand issues",
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
        Shape::Plain => plain(&mut writer, count),
        Shape::Ownership => ownership(&mut writer, count),
        Shape::Churn => churn(&mut writer, count),
    }
    writer.files
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
        let n = self.written;
        let millis = 1_791_558_245_123 + u64::try_from(n).unwrap_or(0);
        format!(
            "{:08x}-{:04x}-7000-8000-{:012x}",
            millis >> 16,
            millis & 0xffff,
            n
        )
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

/// The history `timing --replay` has always measured, draw for draw.
fn plain(w: &mut Writer, count: usize) {
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
            issues.push(w.append(m, "create", None, create(n)));
            continue;
        }
        let issue = issues[w.next(issues.len())].clone();
        let (op, data) = match w.next(10) {
            0..=2 => ("set", edit(n)),
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

fn obj(members: &[(&str, Value)]) -> Value {
    let members: Object = members
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    Value::from(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    use beadroll_core::journal::{Journal, read_attempt, read_segment};
    use beadroll_core::replay::replay;

    fn journal(files: &[Files]) -> Journal {
        let mut journal = Journal::new();
        for machine in files {
            for event in read_segment(&machine.segment, "segment", Some(&mut journal.invalid)) {
                journal.add(event, &machine.replica, "segment", true);
            }
        }
        for machine in files {
            for (name, data) in &machine.attempts {
                for event in read_attempt(data, name, Some(&mut journal.invalid)) {
                    journal.add(event, &machine.replica, name, false);
                }
            }
        }
        journal.settle();
        journal
    }

    #[test]
    fn every_event_of_every_shape_applies() {
        for shape in [Shape::Plain, Shape::Ownership, Shape::Churn] {
            let files = history(3000, shape);
            let journal = journal(&files);
            assert_eq!(journal.events().len(), 3000, "{shape:?}");
            let st = replay(journal).unwrap();
            assert!(st.integrity().is_empty(), "{shape:?}: {:?}", st.integrity());
            assert!(st.pending().is_empty(), "{shape:?}");
            assert!(st.chain_breaks().is_empty(), "{shape:?}");
        }
    }

    #[test]
    fn the_ownership_shape_is_heavy_in_attempts() {
        let files = history(3000, Shape::Ownership);
        let attempted: usize = files
            .iter()
            .flat_map(|machine| &machine.attempts)
            .map(|(_, data)| data.iter().filter(|b| **b == b'\n').count())
            .sum();
        assert!(
            attempted > 1200,
            "{attempted} of 3000 events in attempt files"
        );
        let st = replay(journal(&files)).unwrap();
        let taken_over = st
            .issues()
            .flat_map(|issue| issue.claim_history())
            .filter(|ended| ended.how == "takeover")
            .count();
        assert!(taken_over > 50, "{taken_over} takeovers");
        assert!(
            st.issues()
                .any(|issue| !issue.provisional_claims().is_empty())
        );
    }

    #[test]
    fn the_churn_shape_keeps_its_state_the_size_it_is() {
        let tags = |count: usize| {
            let st = replay(journal(&history(count, Shape::Churn))).unwrap();
            assert_eq!(st.issues().count(), CHURN_ISSUES);
            (st.issues())
                .map(|issue| {
                    issue
                        .registers()
                        .values()
                        .map(|tags| tags.len())
                        .sum::<usize>()
                })
                .sum::<usize>()
        };
        let (fewer, more) = (tags(3000), tags(12_000));
        assert!(more < fewer * 2, "{fewer} register values, then {more}");
    }

    /// The plain history is the one measured before the ownership shape was added, byte for byte
    /// (its digest is pinned), so earlier records stay comparable.
    #[test]
    fn the_plain_history_is_the_one_always_measured() {
        use sha2::{Digest as _, Sha256};
        let mut digest = Sha256::new();
        for machine in history(2000, Shape::Plain) {
            digest.update(machine.replica.as_bytes());
            digest.update(&machine.segment);
            assert!(machine.attempts.is_empty());
        }
        let hex: String = digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            hex,
            "de17de630f0a4b6dfcd50a0797a6a56a93ada20b0f175a1f885ba1549c8dcbfe"
        );
    }
}
