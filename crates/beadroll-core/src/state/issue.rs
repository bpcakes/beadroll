//! One issue as replay keeps it (Python's issue dict): its fields, the registers and tag sets they
//! come from, its comments, and the record of who held it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock};

use crate::event::Field;
use crate::id::EventId;
use crate::json::{Int, Object, Value};
use crate::op::PyInt;

use super::small::{IdMap, IdSet, VecMap};

/// An issue.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub(crate) uid: EventId,
    pub(crate) key: String,
    /// Each field by [`Field::index`]: absent, or its value (`null` included, as Python keeps a
    /// field a create cleared).
    pub(crate) fields: [Option<Value>; 16],
    pub(crate) reg: VecMap<Field, IdMap<Value>>,
    pub(crate) label_tags: VecMap<String, IdSet>,
    pub(crate) dep_tags: VecMap<(String, String), IdMap<Value>>,
    pub(crate) labels: BTreeSet<String>,
    pub(crate) deps: VecMap<(String, String), Value>,
    pub(crate) comments: Vec<Comment>,
    /// Few issues have extras: boxed, so that the others keep a pointer.
    pub(crate) extra: Option<Box<Object>>,
    pub(crate) dangling: Vec<Value>,
    pub(crate) retracted: BTreeMap<(String, Option<String>), IdSet>,
    pub(crate) claim: Option<Box<Claim>>,
    pub(crate) claim_generation: Int,
    pub(crate) claim_history: Vec<EndedClaim>,
    pub(crate) provisional_claims: Vec<Provisional>,
    pub(crate) lost_claims: Vec<LostClaim>,
    pub(crate) stale: Vec<Stale>,
    pub(crate) unconfirmed: Vec<Value>,
    pub(crate) origin: Origin,
    pub(crate) where_: Option<Arc<Value>>,
    pub(crate) closed_on: Option<Arc<Value>>,
}

/// Where an issue was created, kept as the create's machine, actor and place, which events share:
/// [`Issue::origin`] makes the object from them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Origin {
    pub(crate) machine: Arc<str>,
    pub(crate) actor: Option<Arc<Value>>,
    pub(crate) place: Option<Arc<Value>>,
}

/// The extras of an issue that has none.
static NO_EXTRA: LazyLock<Object> = LazyLock::new(Object::new);

impl Issue {
    /// Its id: the id of the create that made it.
    pub fn uid(&self) -> EventId {
        self.uid
    }

    /// Its key (`ex-a1b2`), as the tracker shows it.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// A field's value, if the issue has the field (`null` when a create cleared it).
    pub fn field(&self, field: Field) -> Option<&Value> {
        self.fields.get(field.index()).and_then(Option::as_ref)
    }

    /// A field's value, `None` when it is absent or `null`: Python's `issue.get(name)`.
    pub fn get(&self, field: Field) -> Option<&Value> {
        self.field(field).filter(|value| **value != Value::Null)
    }

    pub(crate) fn set(&mut self, field: Field, value: Option<Value>) {
        if let Some(slot) = self.fields.get_mut(field.index()) {
            *slot = value;
        }
    }

    /// A text field, or `""`.
    fn text(&self, field: Field) -> &str {
        self.get(field).and_then(Value::as_str).unwrap_or_default()
    }

    /// Its title.
    pub fn title(&self) -> &str {
        self.text(Field::Title)
    }

    /// Its status.
    pub fn status(&self) -> &str {
        self.text(Field::Status)
    }

    /// Its type.
    pub fn issue_type(&self) -> &str {
        self.text(Field::IssueType)
    }

    /// Its priority: an integer, or a boolean an old history wrote (`[PTrue]`). Every issue replay
    /// makes has one (a create defaults it to 2, and validation refuses `null`); 2 stands in
    /// otherwise.
    pub fn priority(&self) -> PyInt {
        self.field(Field::Priority)
            .and_then(PyInt::read)
            .unwrap_or_else(|| PyInt::from(2))
    }

    /// Every value of each register two writers may have set without seeing each other, the
    /// registers in the order first written.
    pub fn registers(&self) -> &VecMap<Field, IdMap<Value>> {
        &self.reg
    }

    /// Its labels.
    pub fn labels(&self) -> &BTreeSet<String> {
        &self.labels
    }

    /// The events behind each label, empty once removed, the labels in the order first added.
    pub fn label_tags(&self) -> &VecMap<String, IdSet> {
        &self.label_tags
    }

    /// Its dependencies: `(target, type)` and the metadata of the earliest addition, in the order
    /// first added. A target is an issue's uid, or text that names no issue.
    pub fn deps(&self) -> &VecMap<(String, String), Value> {
        &self.deps
    }

    /// The additions behind each dependency, empty once removed.
    pub fn dep_tags(&self) -> &VecMap<(String, String), IdMap<Value>> {
        &self.dep_tags
    }

    /// Dependencies removed by target and type (`None` for every type), and the removals.
    pub fn retracted(&self) -> &BTreeMap<(String, Option<String>), IdSet> {
        &self.retracted
    }

    /// Its comments, imported first, then those made here in replay order.
    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    /// Fields of an imported record bead does not know, and what a delete records.
    pub fn extra(&self) -> &Object {
        self.extra.as_deref().unwrap_or(&NO_EXTRA)
    }

    /// Its extras, to change.
    pub(crate) fn extra_mut(&mut self) -> &mut Object {
        self.extra.get_or_insert_with(Box::default)
    }

    /// Imported dependencies on keys that are not issues yet.
    pub fn dangling(&self) -> &[Value] {
        &self.dangling
    }

    /// Who holds it, if anyone.
    pub fn claim(&self) -> Option<&Claim> {
        self.claim.as_deref()
    }

    /// How many times it has been claimed, as the highest generation seen.
    pub fn claim_generation(&self) -> &Int {
        &self.claim_generation
    }

    /// Claims that ended, oldest first.
    pub fn claim_history(&self) -> &[EndedClaim] {
        &self.claim_history
    }

    /// Offline claims, which never displace a confirmed owner.
    pub fn provisional_claims(&self) -> &[Provisional] {
        &self.provisional_claims
    }

    /// Claims that lost to the owner of the time.
    pub fn lost_claims(&self) -> &[LostClaim] {
        &self.lost_claims
    }

    /// Ends of claims that were no longer current, ignored.
    pub fn stale(&self) -> &[Stale] {
        &self.stale
    }

    /// This machine's ownership attempts the remote has not confirmed (the store fills these).
    pub fn unconfirmed(&self) -> &[Value] {
        &self.unconfirmed
    }

    /// Where it was created: the machine, the members of the create's actor, and the checkout.
    pub fn origin(&self) -> Object {
        let Origin {
            machine,
            actor,
            place,
        } = &self.origin;
        let mut origin = Object::new();
        origin.insert("machine".into(), Value::from(&**machine));
        for (name, value) in actor
            .as_deref()
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            origin.insert(name.clone(), value.clone());
        }
        if let Some(place) = place {
            origin.insert("where".into(), Value::clone(place));
        }
        origin
    }

    /// Where it was last worked on.
    pub fn where_(&self) -> Option<&Value> {
        self.where_.as_deref()
    }

    /// Where it was closed, until reopened.
    pub fn closed_on(&self) -> Option<&Value> {
        self.closed_on.as_deref()
    }
}

/// The confirmed owner of an issue.
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    /// The claim event.
    pub id: EventId,
    /// Who holds it.
    pub owner: String,
    /// The machine that claimed it.
    pub machine: String,
    /// The agent, as the claim's actor named it.
    pub agent: Option<String>,
    /// The agent's thread.
    pub thread: Option<String>,
    /// The issue's claim count when this claim took it.
    pub generation: Int,
    /// When.
    pub since: String,
    /// `online`, or how the claim says it was made.
    pub mode: String,
    /// The checkout it was made in.
    pub where_: Option<Arc<Value>>,
}

impl Claim {
    /// As Python's dict holds it, in its key order.
    pub fn to_value(&self) -> Object {
        let mut out = Object::new();
        out.insert("id".into(), Value::from(self.id.to_string()));
        out.insert("owner".into(), Value::from(self.owner.as_str()));
        out.insert("machine".into(), Value::from(self.machine.as_str()));
        out.insert("agent".into(), Value::from(self.agent.clone()));
        out.insert("thread".into(), Value::from(self.thread.clone()));
        out.insert("generation".into(), Value::Int(self.generation.clone()));
        out.insert("since".into(), Value::from(self.since.as_str()));
        out.insert("mode".into(), Value::from(self.mode.as_str()));
        if let Some(place) = &self.where_ {
            out.insert("where".into(), Value::clone(place));
        }
        out
    }
}

/// A claim that ended.
#[derive(Clone, Debug, PartialEq)]
pub struct EndedClaim {
    /// The claim.
    pub claim: Claim,
    /// When it ended.
    pub ended: String,
    /// Who ended it.
    pub ended_by: String,
    /// How: `takeover`, `release`, `close`, `reopen` or `delete`.
    pub how: &'static str,
    /// The reason given, if any.
    pub reason: Option<String>,
}

impl EndedClaim {
    /// As Python's dict holds it, in its key order.
    pub fn to_value(&self) -> Object {
        let mut out = self.claim.to_value();
        out.insert("ended".into(), Value::from(self.ended.as_str()));
        out.insert("ended_by".into(), Value::from(self.ended_by.as_str()));
        out.insert("how".into(), Value::from(self.how));
        if let Some(reason) = &self.reason {
            out.insert("reason".into(), Value::from(reason.as_str()));
        }
        out
    }
}

/// An offline claim.
#[derive(Clone, Debug, PartialEq)]
pub struct Provisional {
    /// The claim event.
    pub id: EventId,
    /// Who made it.
    pub owner: String,
    /// When.
    pub since: String,
    /// The checkout it was made in.
    pub where_: Option<Arc<Value>>,
}

impl Provisional {
    /// As Python's dict holds it, in its key order.
    pub fn to_value(&self) -> Object {
        let mut out = Object::new();
        out.insert("id".into(), Value::from(self.id.to_string()));
        out.insert("owner".into(), Value::from(self.owner.as_str()));
        out.insert("since".into(), Value::from(self.since.as_str()));
        if let Some(place) = &self.where_ {
            out.insert("where".into(), Value::clone(place));
        }
        out
    }
}

/// A claim that lost to the owner of the time.
#[derive(Clone, Debug, PartialEq)]
pub struct LostClaim {
    /// Who tried.
    pub by: String,
    /// When.
    pub at: String,
    /// Who held the issue.
    pub held_by: String,
    /// The claim event.
    pub event: EventId,
}

impl LostClaim {
    /// As Python's dict holds it, in its key order.
    pub fn to_value(&self) -> Object {
        let mut out = Object::new();
        out.insert("by".into(), Value::from(self.by.as_str()));
        out.insert("at".into(), Value::from(self.at.as_str()));
        out.insert("held_by".into(), Value::from(self.held_by.as_str()));
        out.insert("event".into(), Value::from(self.event.to_string()));
        out
    }
}

/// An end of a claim that was no longer current, ignored.
#[derive(Clone, Debug, PartialEq)]
pub struct Stale {
    /// `release`, `close` or `delete`.
    pub op: String,
    /// Who.
    pub by: String,
    /// When.
    pub at: String,
    /// The claim it named.
    pub claim: Option<String>,
}

impl Stale {
    /// As Python's dict holds it, in its key order.
    pub fn to_value(&self) -> Object {
        let mut out = Object::new();
        out.insert("op".into(), Value::from(self.op.as_str()));
        out.insert("by".into(), Value::from(self.by.as_str()));
        out.insert("at".into(), Value::from(self.at.as_str()));
        out.insert("claim".into(), Value::from(self.claim.clone()));
        out
    }
}

/// A comment.
#[derive(Clone, Debug, PartialEq)]
pub enum Comment {
    /// A comment made here, by a `comment` event.
    Native {
        /// The comment event.
        uid: EventId,
        /// Its place among the tracker's comments made here.
        seq: u64,
        /// Who wrote it.
        author: String,
        /// What it says.
        text: String,
        /// When.
        created_at: String,
        /// The checkout it was written in.
        where_: Option<Arc<Value>>,
    },
    /// A comment an imported record carried, as Python keeps it: `{"uid": null, ...}`.
    Imported(Object),
}

impl Comment {
    /// As Python's dict holds it, in its key order.
    pub fn to_value(&self) -> Object {
        match self {
            Comment::Native {
                uid,
                seq,
                author,
                text,
                created_at,
                where_,
            } => {
                let mut out = Object::new();
                out.insert("uid".into(), Value::from(uid.to_string()));
                out.insert(
                    "seq".into(),
                    Value::Int(Int::from(i64::try_from(*seq).unwrap_or(i64::MAX))),
                );
                out.insert("author".into(), Value::from(author.as_str()));
                out.insert("text".into(), Value::from(text.as_str()));
                out.insert("created_at".into(), Value::from(created_at.as_str()));
                if let Some(place) = where_ {
                    out.insert("where".into(), Value::clone(place));
                }
                out
            }
            Comment::Imported(members) => members.clone(),
        }
    }

    /// The comment event, for a comment made here.
    pub fn uid(&self) -> Option<EventId> {
        match self {
            Comment::Native { uid, .. } => Some(*uid),
            Comment::Imported(_) => None,
        }
    }

    /// Its member `key`, as Python's `comment.get(key)` reads it.
    pub fn get(&self, key: &str) -> Option<Value> {
        self.to_value().get(key).cloned()
    }
}
