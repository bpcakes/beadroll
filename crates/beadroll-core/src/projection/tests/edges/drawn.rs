//! The edges the differential tests' histories draw beside the generator's own blocks and
//! parent-child additions and removals (roll-lqjj.39), so that holding edges typed is checked on
//! what it changes: targets that name no issue (a key, an external reference, an id in capitals,
//! an id no event has), kinds beyond blocks and parent-child (related, discovered-from, a kind of
//! the user's own, none, and an empty one, both blocks), imports whose dependencies carry metadata
//! with members of their own and wait for keys made later or never, an edge two machines add while
//! neither has seen the other's, an edge removed and added again, and an edge to a renamed copy.

use crate::json::{Object, Value};

use super::super::{World, str_of};

/// The kinds a drawn edge has: `None` and `""` are blocks.
const KINDS: [Option<&str>; 7] = [
    Some("blocks"),
    Some("parent-child"),
    Some("related"),
    Some("discovered-from"),
    Some("tracks"),
    None,
    Some(""),
];

/// Keys an import waits for: made by the generator's creates now and then (`pr-a1`, `pr-c3`), or
/// never (`pr-f6`, `pr-g7`).
const AWAITED: [&str; 4] = ["pr-a1", "pr-c3", "pr-f6", "pr-g7"];

/// Targets that name no issue, as a `dep_add` may name them: a key, an external reference, and an
/// id no event has. An id in capitals is drawn from the issues themselves.
const NAMELESS: [&str; 3] = [
    "pr-b2",
    "jira:OPS-12",
    "01a00000-0000-7000-8000-000000000000",
];

impl World {
    /// The dependencies an import names by key, one to three, each of a kind drawn, now and then
    /// with the members a Beads export gives (`created_at`, `created_by`, `metadata` with members
    /// of its own, `thread_id`) and one bead keeps without reading (`note`).
    pub(in super::super) fn awaited(&mut self, roll: usize) -> Vec<Value> {
        let mut named = Vec::new();
        for n in 0..1 + self.rng.below(3) {
            let mut dep = Object::new();
            let key = AWAITED[self.rng.below(AWAITED.len())];
            dep.insert("depends_on_id".into(), Value::from(key));
            if let Some(kind) = KINDS[self.rng.below(KINDS.len())] {
                dep.insert("type".into(), Value::from(kind));
            }
            if self.rng.chance(60) {
                self.stamped(&mut dep, roll, n);
            }
            if self.rng.chance(20) {
                dep.insert("note".into(), Value::from(format!("kept {roll}.{n}")));
            }
            named.push(Value::from(dep));
        }
        named
    }

    /// A Beads export's members of a dependency, into `dep`: when and by whom it was made, now and
    /// then null, its metadata, a JSON text with members of its own, empty or absent, and its thread.
    fn stamped(&mut self, dep: &mut Object, roll: usize, n: usize) {
        let made = match self.rng.below(4) {
            0 => Value::Null,
            _ => Value::from(format!("2025-11-{:02}T00:00:00Z", 1 + self.rng.below(28))),
        };
        dep.insert("created_at".into(), made);
        let by = ["importer", "seed"][self.rng.below(2)];
        dep.insert("created_by".into(), Value::from(by));
        match self.rng.below(3) {
            0 => {}
            1 => drop(dep.insert("metadata".into(), Value::from("{}"))),
            _ => {
                let metadata = format!("{{\"why\":\"r{roll}\",\"n\":{n}}}");
                dep.insert("metadata".into(), Value::from(metadata));
            }
        }
        if self.rng.chance(50) {
            dep.insert("thread_id".into(), Value::from(format!("th-{n}")));
        }
    }

    /// An edge operation of machine `m` on `issue` (one of `issues`, those it knows), or on the
    /// issue it knows first by id: to a target that names no issue, to a copy renamed for a key
    /// two creates minted alike, the one edge every machine draws, related, from the issue it knows
    /// first to the one it knows second (added or removed, so that two machines add it without seeing each
    /// other's and it is removed and added again), or an import's addition with its record's
    /// metadata; of a kind drawn. Returns the operation, its issue and its data.
    pub(in super::super) fn edge_drawn(
        &mut self,
        m: usize,
        issue: String,
        issues: &[String],
        roll: usize,
    ) -> (&'static str, String, Object) {
        let mut data = Object::new();
        let kind = KINDS[self.rng.below(KINDS.len())];
        data.insert("type".into(), kind.map_or(Value::Null, Value::from));
        let (mut on, mut add) = (issue, true);
        let to = match self.rng.below(5) {
            0 => match self.rng.below(NAMELESS.len() + 1) {
                0 => (self.rng.pick(issues).cloned().unwrap_or_default()).to_uppercase(),
                n => NAMELESS[n - 1].to_owned(),
            },
            1 => match self.renamed(m) {
                Some(copy) => copy,
                None => self.rng.pick(issues).cloned().unwrap_or_default(),
            },
            2 | 3 => {
                let mut first = issues.to_vec();
                first.sort_unstable();
                on = first.first().cloned().unwrap_or_default();
                data.insert("type".into(), Value::from("related"));
                add = self.rng.chance(60);
                first.get(1).unwrap_or(&on).clone()
            }
            _ => {
                let mut meta = Object::new();
                self.stamped(&mut meta, roll, 0);
                data.insert("meta".into(), Value::from(meta));
                self.rng.pick(issues).cloned().unwrap_or_default()
            }
        };
        data.insert("to".into(), Value::from(to));
        if !add {
            return ("dep_remove", on, data);
        }
        if !data.contains_key("meta") {
            data.insert("meta".into(), Value::from(Object::new()));
        }
        ("dep_add", on, data)
    }

    /// The id of a create machine `m` knows whose key an earlier create it knows minted too: a copy
    /// replay renames, or the issue that keeps the key when the copy sorts first.
    fn renamed(&self, m: usize) -> Option<String> {
        let mut minted: Vec<(String, String)> = Vec::new();
        let mut copies = Vec::new();
        for at in &self.machines[m].known {
            let value = &self.events[*at].value;
            if value.get("op").and_then(Value::as_str) != Some("create") {
                continue;
            }
            let key = str_of(value.get("data").and_then(|data| data.get("key")));
            let id = self.id_of(*at);
            if minted.iter().any(|(was, _)| *was == key) {
                copies.push(id.clone());
            }
            minted.push((key, id));
        }
        copies.last().cloned()
    }
}

/// What histories' edges hold of what [`World::awaited`] and [`World::edge_drawn`] draw, counted in
/// every state replay makes of them, batch by batch: an edge is counted in each state it is in.
#[derive(Debug, Default)]
pub(in super::super) struct Exercised {
    /// Edges to a target that names no issue, and those among them to an id in capitals.
    pub(super) nameless: usize,
    pub(super) capitals: usize,
    /// The kinds edges have.
    pub(super) kinds: std::collections::BTreeSet<String>,
    /// Edges an import's create brought to a key made after it.
    pub(super) waited: usize,
    /// Additions whose metadata has a Beads export's `metadata` or `thread_id`.
    pub(super) stamped: usize,
    /// Edges added by two events neither of which had seen the other.
    pub(super) concurrent: usize,
    /// Edges added again by an event that had seen a removal of them.
    pub(super) again: usize,
    /// Edges to a copy replay renamed.
    pub(super) renamed: usize,
}

impl Exercised {
    /// Fails unless the histories counted held each thing drawn.
    pub(in super::super) fn enough(&self) {
        assert!(
            self.nameless > 1_000
                && self.capitals > 500
                && self.kinds.len() == 5
                && self.waited > 500
                && self.stamped > 2_000
                && self.concurrent > 50
                && self.again > 200
                && self.renamed > 5_000,
            "{self:?}"
        );
    }

    /// Counts what `state` holds.
    pub(in super::super) fn count(&mut self, state: &crate::state::State) {
        let renamed: Vec<String> = (state.renames().iter())
            .map(|rename| rename.uid.to_string())
            .collect();
        for issue in state.issues() {
            for ((to, kind), tags) in issue.dep_tags().iter().filter(|(_, t)| !t.is_empty()) {
                let ids: Vec<_> = tags.iter().map(|(id, _)| *id).collect();
                let target = state.issue_named(to);
                self.nameless += usize::from(target.is_none());
                let lower = crate::id::EventId::parse(&to.to_lowercase());
                self.capitals += usize::from(target.is_none() && lower.is_some());
                self.kinds.insert(kind.clone());
                let later = target.is_some_and(|target| target.uid() > issue.uid());
                self.waited += usize::from(ids.contains(&issue.uid()) && later);
                for (_, meta) in tags.iter() {
                    let stamped = meta.get("metadata").or(meta.get("thread_id"));
                    self.stamped += usize::from(stamped.is_some());
                }
                let apart = |a, b| !state.saw(a, b) && !state.saw(b, a);
                let concurrent = (ids.iter().enumerate())
                    .any(|(at, a)| ids[at + 1..].iter().any(|b| apart(*a, *b)));
                self.concurrent += usize::from(concurrent);
                let removals = [(to.clone(), Some(kind.clone())), (to.clone(), None)];
                let again = (removals.iter())
                    .filter_map(|removed| issue.retracted().get(removed))
                    .flat_map(|removals| removals.iter())
                    .any(|removal| ids.iter().any(|id| state.saw(*id, *removal)));
                self.again += usize::from(again);
                self.renamed += usize::from(renamed.contains(to));
            }
        }
    }
}
