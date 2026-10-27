//! Replayed state: issues in replay order, their registers, tags and claims, and event ancestry.

#[cfg(test)]
pub(crate) mod asked;
mod awaiting;
mod claim;
mod edges;
mod issue;
mod issues;
mod runs;
mod seen;
mod small;
mod texts;

use indexmap::IndexMap;

use crate::id::EventId;
use crate::journal::{Event, Journal};
use crate::json::Value;
use crate::key;
use crate::text::str_of;
use crate::work::{Work, tally};

pub(crate) use awaiting::{Awaiting, named as awaited};
pub use claim::{Claim, Ended, EndedClaim, LostClaim, Provisional, Stale};
pub use edges::{DepTags, Deps, DepsIter, Edge, Meta, Target};
pub use issue::{Comment, Issue};
pub(crate) use issue::{Origin, Stamps};
pub(crate) use issues::Issues;
pub(crate) use runs::Runs;
#[cfg(test)]
pub(crate) use runs::used;
pub(crate) use seen::{Arena, Knowledge, Place, raise, reached};
pub use small::{IdMap, IdSet, VecMap};
pub use texts::{Stored, TEXT_FIELDS, TextPlace, Texts, is_text};

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
    /// An integrity row with event, issue and problem, plus origin for a record the reader could
    /// not use.
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
    /// The rename as an object with from, to, replica and uid.
    pub fn to_value(&self) -> crate::json::Object {
        let mut members = crate::json::Object::new();
        members.insert("from".into(), Value::from(self.from.as_str()));
        members.insert("to".into(), Value::from(self.to.as_str()));
        members.insert("replica".into(), Value::from(self.replica.as_str()));
        members.insert("uid".into(), Value::from(self.uid.to_string()));
        members
    }
}

/// Replay cannot finish when all candidate keys for a duplicate are taken. Commands on that
/// tracker return KEY_SPACE.
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
    /// In the order replay created them, with which changed since they were read back
    /// (roll-lqjj.44).
    pub(crate) issues: Issues,
    pub(crate) by_key: IndexMap<String, EventId>,
    pub(crate) renames: Vec<Rename>,
    pub(crate) integrity: Vec<Integrity>,
    pub(crate) pending: Vec<EventId>,
    pub(crate) orphans: u64,
    pub(crate) comment_count: u64,
    pub(crate) last_by_thread: IndexMap<(String, Option<String>), EventId>,
    /// Each parent's children, by the id their parent-child edges name, in replay order: built
    /// once replay ends, when edges no longer change. An edge to text names no parent.
    pub(crate) children: std::collections::HashMap<EventId, Vec<EventId>>,
    /// Where every imported dependency on a key that is not an issue's yet is, by that key: a new
    /// key visits the entries that wait for it and no others (roll-lqjj.18). Derived from the
    /// issues' lists, so a state read back builds it again rather than reading it.
    pub(crate) awaiting: Awaiting,
    /// The numbers renamed copies found claimed, in runs under each stem, which the next copy of a
    /// numbered key skips (roll-lqjj.46). Derived from what copies asked, and never written: a
    /// state read back starts with none.
    pub(crate) runs: Runs,
    /// The text file its issues' texts are placed in, as the store that read it names it
    /// (roll-lqjj.6); `None` for a state replayed.
    pub(crate) texts_file: Option<[u8; 16]>,
}

impl State {
    pub(crate) fn new(knowledge: Knowledge) -> State {
        State {
            knowledge,
            prefix: None,
            name: None,
            issues: Issues::default(),
            by_key: IndexMap::new(),
            renames: Vec::new(),
            integrity: Vec::new(),
            pending: Vec::new(),
            orphans: 0,
            comment_count: 0,
            last_by_thread: IndexMap::new(),
            children: std::collections::HashMap::new(),
            awaiting: Awaiting::default(),
            runs: Runs::default(),
            texts_file: None,
        }
    }

    /// The text file its issues' texts are placed in, as the store names it; `None` for a state
    /// replayed, whose texts are all in it.
    pub fn texts_file(&self) -> Option<[u8; 16]> {
        self.texts_file
    }

    /// Names the text file its issues' texts are placed in.
    pub fn set_texts_file(&mut self, file: Option<[u8; 16]>) {
        self.texts_file = file;
    }

    /// Whether `event` was among the events it was replayed from; of a projection's state, among
    /// those it knows ([`crate::projection::Projection::known`]), and of a state read alone, among
    /// those whose writers it holds ([`crate::projection::decode_state`]).
    pub fn holds(&self, event: EventId) -> bool {
        self.knowledge.events.contains_key(&event)
    }

    /// The events it was replayed from, by id, in their journal's order: of a projection's state or
    /// a state read alone, those [`State::holds`] says.
    pub fn events(&self) -> impl Iterator<Item = EventId> {
        self.knowledge.events.keys().copied()
    }

    /// The tracker's prefix, as its first `init` set it: any JSON value, rendered with `str_of` where a
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

    /// The issue a dependency on `to` depends on, if `to` is an issue's id.
    pub fn target(&self, to: &Target) -> Option<&Issue> {
        to.id().and_then(|uid| self.issue(uid))
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

    /// Whether the writer of `event` had already seen `other`.
    pub fn saw(&self, event: EventId, other: EventId) -> bool {
        self.knowledge.saw(event, other)
    }

    /// The issue named by a full key or bare suffix.
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

    /// The key of issue `uid`, or `uid` itself if it names no issue.
    pub fn key_of<'a>(&'a self, uid: &'a str) -> &'a str {
        self.issue_named(uid).map_or(uid, Issue::key)
    }

    /// The key of the issue `to` names, or the text it was named by when it names no issue.
    pub fn target_key(&self, to: &Target) -> std::borrow::Cow<'_, str> {
        match self.target(to) {
            Some(issue) => issue.key().into(),
            None => to.to_string().into(),
        }
    }

    /// The actor who wrote `event`, formatted as an actor label.
    pub fn who(&self, event: EventId) -> String {
        self.knowledge.who(event).unwrap_or_default()
    }

    /// Keys a new or renamed issue must not take: every issue's, and every key an imported dependency
    /// is still waiting for (the record it names may be imported later).
    pub fn claimed_keys(&self) -> std::collections::HashSet<&str> {
        let mut taken: std::collections::HashSet<&str> =
            self.by_key.keys().map(String::as_str).collect();
        for issue in self.issues.values() {
            // an entry taken out while events apply is null, and names no key
            taken.extend(issue.dangling.iter().filter_map(awaited));
        }
        tally(Work::Key, taken.len());
        taken
    }

    /// The key assigned to duplicate issue `uid` when `key` is taken: the one [`key::free_key`]
    /// picks. A numbered key's copy counts upward past the runs of numbers earlier copies found
    /// claimed, a run at a time ([`Runs`], roll-lqjj.46), and keeps the numbers it finds claimed
    /// for the next; so the N copies two machines make of `ex-1.1` to `ex-1.N` ask of 3N−1 keys,
    /// not N(N+1). In this crate's tests every key it picks is checked against the rule counting
    /// one number at a time.
    pub(crate) fn free_key(&mut self, key: &str, uid: EventId) -> Result<String, KeySpace> {
        let free = match key::split_number(key) {
            Some((stem, digits)) => {
                let mut runs = std::mem::take(&mut self.runs);
                let free = runs.free(stem, digits, |candidate| self.claims_key(candidate));
                self.runs = runs;
                Some(free)
            }
            None => key::free_key(key, &uid.to_string(), &|candidate| {
                self.claims_key(candidate)
            }),
        };
        #[cfg(test)]
        runs::agrees(self, key, uid, free.as_deref());
        free.ok_or_else(|| KeySpace {
            key: key.to_owned(),
        })
    }

    /// Whether [`State::claimed_keys`] holds `key`: an issue's, or one an imported dependency still
    /// waits for, asked of the key alone. Replay asks it of every key it tries for a copy two
    /// machines minted alike, which collecting every key for each copy made cost the whole tracker
    /// a copy (X03 in `docs/FINDINGS.md`).
    pub(crate) fn claims_key(&self, key: &str) -> bool {
        tally(Work::Key, 1);
        let claimed = self.by_key.contains_key(key) || self.awaiting.names(key);
        #[cfg(test)]
        asked::agrees(self, key, claimed);
        claimed
    }

    /// A key held by neither an issue nor an imported dependency. Children count upward from
    /// siblings; other keys use the prefix and seed digest.
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
        let children = (self.children.get(&uid))
            .map(Vec::as_slice)
            .unwrap_or_default();
        tally(Work::Edge, children.len());
        children
    }

    /// Lists a local unconfirmed ownership attempt without making it count as ownership. Returns
    /// false if it names no issue.
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

    /// Makes whole every list of imported dependencies an entry was taken from while events
    /// applied ([`Awaiting::settle`]).
    pub(crate) fn settle_awaiting(&mut self) {
        self.awaiting.settle(&mut self.issues);
    }

    /// Indexes the parent-child edges, once replay has settled them.
    pub(crate) fn index_children(&mut self) {
        let mut children: std::collections::HashMap<EventId, Vec<EventId>> =
            std::collections::HashMap::new();
        tally(Work::Index, self.issues.len());
        for child in self.issues.values() {
            for edge in child.deps().keys() {
                if let (Some(parent), "parent-child") = (edge.to().id(), edge.kind()) {
                    children.entry(parent).or_default().push(child.uid);
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
            .deps()
            .keys()
            .filter(|edge| crate::query::blocking(edge.kind()))
            .filter_map(|edge| edge.to().id().filter(|to| self.issues.contains_key(to)))
            .collect();
        found.extend(self.children(uid).iter().copied());
        found.into_iter().collect()
    }
}

/// An event's actor label: its explicit name, else agent:thread@machine or agent@machine.
pub fn actor_of(event: &Event) -> String {
    actor_label(event.actor(), event.replica())
}

/// The event's actor label, using an empty actor when absent or falsy.
pub fn actor_label(actor: Option<&Value>, replica: &str) -> String {
    let member = |key: &str| actor.and_then(|actor| actor.get(key));
    if let Some(name) = member("name").filter(|name| name.truthy()) {
        return str_of(name);
    }
    // Replay names the writer of every event it applies: built in place, `str_of` only for what
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

/// A state is immutable once replayed and shared between threads behind an `Arc`, as studio
/// shares it.
const _: () = {
    const fn shareable<T: Send + Sync>() {}
    shareable::<State>();
    shareable::<Journal>();
};
