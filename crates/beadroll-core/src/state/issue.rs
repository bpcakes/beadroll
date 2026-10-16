//! An issue's replayed fields, their registers and tag sets, comments and ownership history.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock};

use crate::event::Field;
use crate::id::EventId;
use crate::json::{Int, Object, Value};
use crate::op::IntegerField;
use crate::time::event_time;
use crate::work::{Work, tally};

use super::claim::{Claim, EndedClaim, LostClaim, Provisional, Stale};
use super::small::{IdMap, IdSet, VecMap};
use super::texts::{Texts, is_text};

/// An issue.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub(crate) uid: EventId,
    pub(crate) key: String,
    /// What it shows that no register holds. Every other field is shown from its register
    /// ([`Issue::field`]), which holds each value once (roll-lqjj.10, roll-lqjj.41).
    pub(crate) stamps: Stamps,
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
    /// Where its long fields, their registers' values, its comments and its extras are
    /// (roll-lqjj.6): in it, or only in a projection's text file.
    pub(crate) texts: Texts,
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

/// What an issue shows that no register holds: when and by whom it was created and when an event
/// last touched it, as its create gave them or they defaulted to, and for `closed_at` and
/// `close_reason`, which a set may write into registers of their own, whether that register or
/// the status register was written last, which is what each shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Stamps {
    pub(crate) created_at: Value,
    /// Null for an import that did not say: the record's author is unknown, not the importer.
    pub(crate) created_by: Value,
    pub(crate) updated_at: Value,
    /// Whether `closed_at` shows its own register's value rather than the status register's member.
    pub(crate) own_closed_at: bool,
    /// The same for `close_reason`.
    pub(crate) own_close_reason: bool,
}

impl Stamps {
    /// Whether `field` (`closed_at` or `close_reason`) shows its own register.
    pub(crate) fn own(&self, field: Field) -> bool {
        match field {
            Field::ClosedAt => self.own_closed_at,
            Field::CloseReason => self.own_close_reason,
            _ => false,
        }
    }

    /// Records which register of `field` was written last: its own (`own`) or the status's.
    pub(crate) fn wrote(&mut self, field: Field, own: bool) {
        match field {
            Field::ClosedAt => self.own_closed_at = own,
            Field::CloseReason => self.own_close_reason = own,
            _ => {}
        }
    }
}

/// What a field a create did not give shows while no register holds it: `""`, 2 and `task` for
/// the title, priority and issue_type every issue shows, nothing for the rest. Status has no
/// default: a create always makes its register.
fn default(field: Field) -> Option<&'static Value> {
    static TITLE: LazyLock<Value> = LazyLock::new(|| Value::from(""));
    static PRIORITY: LazyLock<Value> = LazyLock::new(|| Value::from(2));
    static ISSUE_TYPE: LazyLock<Value> = LazyLock::new(|| Value::from("task"));
    match field {
        Field::Title => Some(&TITLE),
        Field::Priority => Some(&PRIORITY),
        Field::IssueType => Some(&ISSUE_TYPE),
        _ => None,
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

    /// A field's value, if the issue has the field (`null` when a create cleared it), shown from
    /// where it is kept once: its register's entry of the largest id, absent when that entry is a
    /// later write's null (roll-lqjj.10, roll-lqjj.41); status, and closed_at and close_reason
    /// unless a set wrote one after the status, from the members of the status register's entry;
    /// the create's default while a field it did not give has no register; and created_at,
    /// created_by and updated_at from the issue's own stamps.
    pub fn field(&self, field: Field) -> Option<&Value> {
        match field {
            Field::CreatedAt => return Some(&self.stamps.created_at),
            Field::CreatedBy => return Some(&self.stamps.created_by),
            Field::UpdatedAt => return Some(&self.stamps.updated_at),
            Field::Status => return self.status_member("status"),
            Field::ClosedAt | Field::CloseReason if !self.stamps.own(field) => {
                return self.status_member(field.name());
            }
            _ => {}
        }
        let Some(register) = self.reg.get(&field) else {
            return default(field);
        };
        let (id, value) = register.last()?;
        // The create's own value, null included, shows until a write replaces it. A write that had
        // not seen the create keeps the create's entry beside its own: the create's null, the
        // larger, then shows for a long field (roll-lqjj.10) and not at all for a short one, as
        // replay showed a short field once a write had touched its register.
        let created = *id == self.uid && (is_text(field) || register.len() == 1);
        (*value != Value::Null || created).then_some(value)
    }

    /// The member `name` of the status register's entry of the largest id.
    fn status_member(&self, name: &str) -> Option<&Value> {
        self.reg.get(&Field::Status)?.last()?.1.get(name)
    }

    /// A field's value, or `None` when absent or null.
    pub fn get(&self, field: Field) -> Option<&Value> {
        self.field(field).filter(|value| **value != Value::Null)
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
    pub fn priority(&self) -> IntegerField {
        self.field(Field::Priority)
            .and_then(IntegerField::read)
            .unwrap_or_else(|| IntegerField::from(2))
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
        tally(Work::Edge, self.dep_tags.len());
        Deps(&self.dep_tags)
    }

    /// The additions behind each dependency, empty once removed.
    pub fn dep_tags(&self) -> &VecMap<(String, String), IdMap<Value>> {
        tally(Work::Edge, self.dep_tags.len());
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

    /// Where its texts are: in it, or in a projection's text file, not read; its long fields,
    /// their registers, its comments and its extras are empty until they are read in.
    pub fn texts(&self) -> &Texts {
        &self.texts
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
    /// An imported comment, with uid defaulting to null.
    Imported(Object),
}

impl Comment {
    /// As a JSON object, in field order.
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

    /// The member `key`, if present.
    pub fn get(&self, key: &str) -> Option<Value> {
        self.to_value().get(key).cloned()
    }
}
