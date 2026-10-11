//! What replay builds (Python's `State` and its issue dicts): the issues in replay order, with every
//! register, tag and claim record Python keeps, and what every event had seen.

mod issue;
mod seen;
mod small;

use indexmap::IndexMap;

use crate::id::EventId;
use crate::journal::{Event, Journal};
use crate::json::Value;
use crate::key;
use crate::text::str_of;

pub(crate) use issue::Origin;
pub use issue::{Claim, Comment, EndedClaim, Issue, LostClaim, Provisional, Stale};
pub(crate) use seen::{Knowledge, Place, raise, reached};
pub use small::{IdMap, IdSet, VecMap};

/// A problem replay reports: an unusable record, or an event it could not apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Integrity {
    /// The event's id, when one could be read.
    pub event: Option<String>,
    /// The key of the issue it is about (its id when that is not an issue).
    pub issue: Option<String>,
    /// What is wrong.
    pub problem: String,
    /// Where it was found, for a record a reader could not use.
    pub origin: Option<String>,
}

/// A copy that kept its key under another name: two machines minted the same key before they
/// synced, and the create that sorted second gave way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rename {
    /// The key both minted.
    pub from: String,
    /// The key the second copy has.
    pub to: String,
    /// The machine that wrote the second copy.
    pub replica: String,
    /// The second copy's issue.
    pub uid: EventId,
}

impl Integrity {
    /// The row as Python holds it: `event`, `issue`, `problem`, and `origin` for a record a reader
    /// could not use.
    pub fn to_value(&self) -> crate::json::Object {
        let mut members = crate::json::Object::new();
        members.insert("event".into(), self.event.clone().into());
        members.insert("issue".into(), self.issue.clone().into());
        members.insert("problem".into(), Value::from(self.problem.as_str()));
        if let Some(origin) = &self.origin {
            members.insert("origin".into(), Value::from(origin.as_str()));
        }
        members
    }
}

impl Rename {
    /// The rename as Python holds it: `from`, `to`, `replica`, `uid`.
    pub fn to_value(&self) -> crate::json::Object {
        let mut members = crate::json::Object::new();
        members.insert("from".into(), Value::from(self.from.as_str()));
        members.insert("to".into(), Value::from(self.to.as_str()));
        members.insert("replica".into(), Value::from(self.replica.as_str()));
        members.insert("uid".into(), Value::from(self.uid.to_string()));
        members
    }
}

/// Why replay cannot finish: every free key a renamed copy could take is taken. Python raises it
/// out of replay, so every command on such a tracker fails with `KEY_SPACE`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot derive a free key from {key}")]
pub struct KeySpace {
    /// The key whose copy has nowhere to go.
    pub key: String,
}

/// Current state, as any machine holding the same events derives it.
#[derive(Clone, Debug)]
pub struct State {
    pub(crate) knowledge: Knowledge,
    pub(crate) prefix: Option<Value>,
    pub(crate) name: Option<Value>,
    /// Boxed, so that what a map of them keeps spare as it grows is a pointer a place, not an issue.
    pub(crate) issues: IndexMap<EventId, Box<Issue>>,
    pub(crate) by_key: IndexMap<String, EventId>,
    pub(crate) renames: Vec<Rename>,
    pub(crate) integrity: Vec<Integrity>,
    pub(crate) pending: Vec<EventId>,
    pub(crate) orphans: u64,
    pub(crate) comment_count: u64,
    pub(crate) last_by_thread: IndexMap<(String, Option<String>), EventId>,
    /// Each parent's children, by the text their parent-child edges name, in replay order: built
    /// once replay ends, when edges no longer change.
    pub(crate) children: std::collections::HashMap<String, Vec<EventId>>,
    /// The issues that may still hold imported dependencies on keys that were not issues yet, in
    /// the order created. Only a create gives an issue such dependencies, so a new key need look
    /// at these alone, not at every issue.
    pub(crate) awaiting: Vec<EventId>,
}

impl State {
    pub(crate) fn new(journal: Journal) -> State {
        State {
            knowledge: Knowledge::new(journal),
            prefix: None,
            name: None,
            issues: IndexMap::new(),
            by_key: IndexMap::new(),
            renames: Vec::new(),
            integrity: Vec::new(),
            pending: Vec::new(),
            orphans: 0,
            comment_count: 0,
            last_by_thread: IndexMap::new(),
            children: std::collections::HashMap::new(),
            awaiting: Vec::new(),
        }
    }

    /// The journal it was replayed from.
    pub fn journal(&self) -> &Journal {
        &self.knowledge.journal
    }

    /// The tracker's prefix, as its first `init` set it: any JSON value, read with `str()` where a
    /// key is made of it.
    pub fn prefix(&self) -> Option<&Value> {
        self.prefix.as_ref()
    }

    /// The tracker's name, set with its prefix.
    pub fn name(&self) -> Option<&Value> {
        self.name.as_ref()
    }

    /// The issues, in the order replay created them.
    pub fn issues(&self) -> impl Iterator<Item = &Issue> {
        self.issues.values().map(|issue| &**issue)
    }

    /// The issue `uid`, if there is one.
    pub fn issue(&self, uid: EventId) -> Option<&Issue> {
        self.issues.get(&uid).map(|issue| &**issue)
    }

    /// The issue whose uid is the text `uid`, if there is one: a dependency names its target so.
    pub fn issue_named(&self, uid: &str) -> Option<&Issue> {
        EventId::parse(uid).and_then(|uid| self.issue(uid))
    }

    /// The issues, in bytewise key order.
    pub fn issues_by_key(&self) -> Vec<&Issue> {
        let mut issues: Vec<&Issue> = self.issues().collect();
        issues.sort_by(|a, b| a.key().cmp(b.key()));
        issues
    }

    /// Every key and the issue that has it, in the order they were taken.
    pub fn keys(&self) -> &IndexMap<String, EventId> {
        &self.by_key
    }

    /// Copies that gave way to another with the same key.
    pub fn renames(&self) -> &[Rename] {
        &self.renames
    }

    /// Everything replay could not use, in the order found.
    pub fn integrity(&self) -> &[Integrity] {
        &self.integrity
    }

    /// Events waiting for causes this journal does not hold, in journal order.
    pub fn pending(&self) -> &[EventId] {
        &self.pending
    }

    /// Events about issues that do not exist.
    pub fn orphans(&self) -> u64 {
        self.orphans
    }

    /// Comments made here, counted in replay order.
    pub fn comment_count(&self) -> u64 {
        self.comment_count
    }

    /// The last applied event of each machine and agent thread.
    pub fn last_by_thread(&self) -> &IndexMap<(String, Option<String>), EventId> {
        &self.last_by_thread
    }

    /// Positions in each machine's journal where an event did not build on its predecessor.
    pub fn chain_breaks(&self) -> &IndexMap<String, Vec<usize>> {
        &self.knowledge.chain_breaks
    }

    /// Whether the writer of `event` had already seen `other` (Python's `State.saw`).
    pub fn saw(&self, event: EventId, other: EventId) -> bool {
        self.knowledge.saw(event, other)
    }

    /// The issue a key or a bare suffix names (Python's `State.resolve`).
    pub fn resolve(&self, key: &str) -> Option<&Issue> {
        let uid = self.by_key.get(key).or_else(|| {
            let prefix = self
                .prefix
                .as_ref()
                .map_or_else(|| "None".to_owned(), str_of);
            self.by_key.get(&format!("{prefix}-{key}"))
        })?;
        self.issue(*uid)
    }

    /// The key of the issue `uid` names, or `uid` itself when that is not an issue
    /// (Python's `State.key`).
    pub fn key_of<'a>(&'a self, uid: &'a str) -> &'a str {
        self.issue_named(uid).map_or(uid, Issue::key)
    }

    /// Who wrote `event`, as bead names an actor (Python's `State.who`).
    pub fn who(&self, event: EventId) -> String {
        self.journal()
            .event(event)
            .map_or_else(String::new, actor_of)
    }

    /// Keys a new or renamed issue must not take: every issue's, and every key an imported dependency
    /// is still waiting for (the record it names may be imported later).
    pub fn claimed_keys(&self) -> std::collections::HashSet<&str> {
        let mut taken: std::collections::HashSet<&str> =
            self.by_key.keys().map(String::as_str).collect();
        for issue in self.issues.values() {
            taken.extend(
                issue
                    .dangling
                    .iter()
                    .filter_map(|dep| dep.get("depends_on_id")?.as_str()),
            );
        }
        taken
    }

    /// The key a copy of `key` (issue `uid`) keeps when `key` is taken (Python's `State.free_key`).
    pub fn free_key(&self, key: &str, uid: EventId) -> Result<String, KeySpace> {
        let taken = self.claimed_keys();
        key::free_key(key, &uid.to_string(), &|candidate| {
            taken.contains(candidate)
        })
        .ok_or_else(|| KeySpace {
            key: key.to_owned(),
        })
    }

    /// A key no issue has and no imported dependency waits for (Python's `mint_key`): a child of
    /// `parent` counts on from its siblings; anything else is the prefix and a digest of `seed`.
    pub fn mint_key(&self, seed: &str, parent: Option<&Issue>) -> Result<String, key::MintError> {
        let taken = self.claimed_keys();
        let taken: Vec<&str> = taken.into_iter().collect();
        let prefix = self
            .prefix
            .as_ref()
            .map_or_else(|| "None".to_owned(), str_of);
        key::mint_key(&prefix, seed, parent.map(Issue::key), &taken)
    }

    /// The children of `uid`: issues with a parent-child edge to it, in replay order.
    pub fn children(&self, uid: EventId) -> &[EventId] {
        self.children
            .get(&uid.to_string())
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Lists one of this machine's unconfirmed ownership attempts on its issue, counting for
    /// nothing (what Python's `view` adds to a replay). `false` when it names no issue.
    pub fn add_unconfirmed(&mut self, event: &Value) -> bool {
        let uid = event
            .get("issue")
            .and_then(Value::as_str)
            .and_then(EventId::parse);
        let Some(issue) = uid.and_then(|uid| self.issues.get_mut(&uid)) else {
            return false;
        };
        let id = event.get("id").and_then(Value::as_str).unwrap_or_default();
        let replica = event
            .get("replica")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let millis = EventId::parse(id).map_or(0, EventId::millis);
        let mut entry = crate::json::Object::new();
        entry.insert("event".into(), Value::from(id));
        entry.insert("op".into(), event.get("op").cloned().unwrap_or(Value::Null));
        let actor = event.get("actor").filter(|actor| actor.truthy());
        entry.insert("by".into(), Value::from(actor_label(actor, replica)));
        entry.insert("at".into(), Value::from(crate::time::event_time(millis)));
        if let Some(place) = event.get("where").filter(|place| place.truthy()) {
            entry.insert("where".into(), place.clone());
        }
        issue.unconfirmed.push(Value::from(entry));
        true
    }

    /// Indexes the parent-child edges, once replay has settled them.
    pub(crate) fn index_children(&mut self) {
        let mut children: std::collections::HashMap<String, Vec<EventId>> =
            std::collections::HashMap::new();
        for child in self.issues.values() {
            for (to, kind) in child.deps.keys() {
                if kind == "parent-child" {
                    children.entry(to.clone()).or_default().push(child.uid);
                }
            }
        }
        self.children = children;
    }

    /// Edges cycle checks follow: `blocks` targets that are issues, plus parent to child as in br,
    /// sorted.
    pub fn waits_for(&self, uid: EventId) -> Vec<EventId> {
        let Some(issue) = self.issues.get(&uid) else {
            return Vec::new();
        };
        let mut found: std::collections::BTreeSet<EventId> = issue
            .deps
            .keys()
            .filter(|(_, kind)| crate::query::blocking(kind))
            .filter_map(|(to, _)| EventId::parse(to).filter(|to| self.issues.contains_key(to)))
            .collect();
        found.extend(self.children(uid).iter().copied());
        found.into_iter().collect()
    }
}

/// How bead names the actor of an event (Python's `actor_label`): its name, or `agent:thread@machine`,
/// or `agent@machine`.
pub fn actor_of(event: &Event) -> String {
    actor_label(event.actor(), event.replica())
}

/// Python's `actor_label(event.get("actor") or {}, replica)`.
pub fn actor_label(actor: Option<&Value>, replica: &str) -> String {
    let member = |key: &str| actor.and_then(|actor| actor.get(key));
    if let Some(name) = member("name").filter(|name| name.truthy()) {
        return str_of(name);
    }
    // Replay names the writer of every event it applies: built in place, `str()` only for what
    // is not text already.
    let push = |label: &mut String, value: &Value| match value {
        Value::Str(text) => label.push_str(text),
        other => label.push_str(&str_of(other)),
    };
    let mut label = String::with_capacity(24 + replica.len());
    match member("agent") {
        Some(agent) => push(&mut label, agent),
        None => label.push_str("human"),
    }
    if let Some(thread) = member("thread").filter(|thread| thread.truthy()) {
        label.push(':');
        push(&mut label, thread);
    }
    label.push('@');
    label.push_str(replica);
    label
}

/// A state is immutable once replayed and shared between threads behind an `Arc` (§2.4).
const _: () = {
    const fn shareable<T: Send + Sync>() {}
    shareable::<State>();
    shareable::<Journal>();
};
