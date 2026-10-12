//! One issue as replay keeps it (Python's issue dict): its fields, the registers and tag sets they
//! come from, its comments, and the record of who held it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock};

use crate::event::Field;
use crate::id::EventId;
use crate::json::{Int, Object, Value};
use crate::op::PyInt;
use crate::time::event_time;

use super::claim::{Claim, EndedClaim, LostClaim, Provisional, Stale};
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

/// An issue's dependencies ([`Issue::deps`]), read from the additions behind them, which hold the
/// metadata each was added with: a removed dependency has none left.
#[derive(Clone, Copy)]
pub struct Deps<'a>(&'a VecMap<(String, String), IdMap<Value>>);

impl<'a> Deps<'a> {
    /// Each dependency and the metadata of its earliest addition.
    pub fn iter(self) -> DepsIter<'a> {
        let earliest: fn(_) -> _ = earliest;
        self.0.into_iter().filter_map(earliest)
    }

    /// Each dependency.
    pub fn keys(self) -> impl Iterator<Item = &'a (String, String)> {
        self.iter().map(|(edge, _)| edge)
    }

    /// The metadata of the earliest addition of `edge`, if the issue has that dependency.
    pub fn get(self, edge: &(String, String)) -> Option<&'a Value> {
        (self.0.get(edge)?.first()).map(|(_, meta)| meta)
    }

    /// Whether the issue has the dependency `edge`.
    pub fn contains_key(self, edge: &(String, String)) -> bool {
        self.get(edge).is_some()
    }

    /// How many dependencies the issue has.
    pub fn len(self) -> usize {
        self.iter().count()
    }

    /// Whether it has none.
    pub fn is_empty(self) -> bool {
        self.iter().next().is_none()
    }
}

impl<'a> IntoIterator for Deps<'a> {
    type Item = (&'a (String, String), &'a Value);
    type IntoIter = DepsIter<'a>;

    fn into_iter(self) -> DepsIter<'a> {
        self.iter()
    }
}

/// The iterator over [`Deps`].
pub type DepsIter<'a> = std::iter::FilterMap<
    <&'a VecMap<(String, String), IdMap<Value>> as IntoIterator>::IntoIter,
    fn((&'a (String, String), &'a IdMap<Value>)) -> Option<(&'a (String, String), &'a Value)>,
>;

/// A dependency and the metadata of its earliest addition, if it has one left.
fn earliest<'a>(
    (edge, tags): (&'a (String, String), &'a IdMap<Value>),
) -> Option<(&'a (String, String), &'a Value)> {
    tags.first().map(|(_, meta)| (edge, meta))
}

impl std::fmt::Debug for Deps<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
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
    pub fn deps(&self) -> Deps<'_> {
        Deps(&self.dep_tags)
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

/// A comment.
#[derive(Clone, Debug, PartialEq)]
pub enum Comment {
    /// A comment made here, by a `comment` event, whose time is when it was written.
    Native {
        /// The comment event.
        uid: EventId,
        /// Its place among the tracker's comments made here.
        seq: u64,
        /// Who wrote it.
        author: Arc<str>,
        /// What it says.
        text: String,
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
                where_,
            } => {
                let mut out = Object::new();
                out.insert("uid".into(), Value::from(uid.to_string()));
                out.insert(
                    "seq".into(),
                    Value::Int(Int::from(i64::try_from(*seq).unwrap_or(i64::MAX))),
                );
                out.insert("author".into(), Value::from(&**author));
                out.insert("text".into(), Value::from(text.as_str()));
                out.insert("created_at".into(), Value::from(event_time(uid.millis())));
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
