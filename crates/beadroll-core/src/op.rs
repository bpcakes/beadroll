//! An event's typed operation. Payloads are read only after
//! validation; invalid events are never applied. A member of the wrong type therefore reads as
//! absent, and missing or falsy members use operation-specific defaults.

use std::cmp::Ordering;
use std::fmt;

use crate::event::Field;
use crate::id::EventId;
use crate::json::{Int, Object, Value};

/// An integer field read from a format-1 event, retaining a legacy boolean representation.
/// Replay accepts booleans in integer fields of old histories (`docs/REVIEWING.md` §2).
/// Comparison and display use their numeric values (`true` is 1, `false` is 0), while JSON keeps
/// the original type.
#[derive(Clone, Debug)]
pub enum IntegerField {
    /// `true` or `false`.
    Bool(bool),
    /// An integer.
    Int(Int),
}

impl IntegerField {
    /// The integer `value` holds, if it holds one (a boolean is one).
    pub fn read(value: &Value) -> Option<IntegerField> {
        match value {
            Value::Bool(flag) => Some(IntegerField::Bool(*flag)),
            Value::Int(int) => Some(IntegerField::Int(int.clone())),
            _ => None,
        }
    }

    /// Its number: `True` is 1.
    pub fn number(&self) -> Int {
        match self {
            IntegerField::Bool(flag) => Int::from(i64::from(*flag)),
            IntegerField::Int(int) => int.clone(),
        }
    }

    /// As JSON.
    pub fn to_value(&self) -> Value {
        match self {
            IntegerField::Bool(flag) => Value::Bool(*flag),
            IntegerField::Int(int) => Value::Int(int.clone()),
        }
    }

    /// Whether it is zero or `false`.
    pub fn is_zero(&self) -> bool {
        self.number() == Int::from(0)
    }
}

impl From<i64> for IntegerField {
    fn from(value: i64) -> IntegerField {
        IntegerField::Int(Int::from(value))
    }
}

impl PartialEq for IntegerField {
    fn eq(&self, other: &IntegerField) -> bool {
        self.number() == other.number()
    }
}

impl Eq for IntegerField {}

impl PartialOrd for IntegerField {
    fn partial_cmp(&self, other: &IntegerField) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IntegerField {
    fn cmp(&self, other: &IntegerField) -> Ordering {
        self.number().cmp(&other.number())
    }
}

impl fmt::Display for IntegerField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.number())
    }
}

/// What a create carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Create {
    /// The key it asks for.
    pub key: String,
    /// The fields it sets, in the order it lists them.
    pub fields: Vec<(Field, Value)>,
    /// Its labels.
    pub labels: Vec<String>,
    /// An imported record's fields bead does not know.
    pub extra: Object,
    /// An imported record's comments, each as it came.
    pub comments: Vec<Object>,
    /// An imported record's dependencies on keys that are not issues yet.
    pub dangling: Vec<Value>,
    /// Whether it came from an import.
    pub imported: bool,
    /// The parent `bead create --parent` made it under, by id: the issue whose key its own was
    /// minted under, as its writer saw the tracker, which it follows when a merge renames that
    /// issue. Absent from what released versions wrote, and from an import's; a value that is no
    /// event id is read as absent, as replay applied such a create before the member was added.
    pub parent: Option<EventId>,
}

/// The `mode` of a provisional claim: one made offline, standing until it is published.
pub const PROVISIONAL: &str = "offline";

/// What a claim carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Claiming {
    /// [`PROVISIONAL`] for a provisional claim; `online` when absent or empty.
    pub mode: Option<String>,
    /// The claim it takes over.
    pub takeover: Option<String>,
    /// Why, for a takeover.
    pub reason: Option<String>,
    /// The generation its writer saw.
    pub generation: Option<IntegerField>,
}

impl Claiming {
    /// Whether it is provisional: claimed offline, standing until it is published and confirmed.
    pub fn provisional(&self) -> bool {
        self.mode.as_deref() == Some(PROVISIONAL)
    }

    /// The claim it takes over, by its event id, when it is a takeover: an empty one names none.
    pub fn takeover_of(&self) -> Option<&str> {
        self.takeover.as_deref().filter(|named| !named.is_empty())
    }
}

/// What a release, close or delete carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Ending {
    /// The claim it ends, by its event id.
    pub claim: Option<String>,
    /// Why.
    pub reason: Option<String>,
}

/// An event's operation and its payload. A journal keeps one for each event, so what few events
/// carry is boxed, and the operation stays small (roll-dih8.11).
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// The tracker's prefix and name; the first `init` in causal order sets them.
    Init {
        /// The prefix: any JSON value, rendered with `str_of` where a key is made of it.
        prefix: Option<Box<Value>>,
        /// The name.
        name: Option<Box<Value>>,
    },
    /// A new issue.
    Create(Box<Create>),
    /// Fields set.
    Set(Vec<(Field, Value)>),
    /// A claim.
    Claim(Box<Claiming>),
    /// The end of a claim by its owner.
    Release(Ending),
    /// The issue closed.
    Close(Ending),
    /// The issue reopened, and why, when its writer said.
    Reopen {
        /// Why.
        reason: Option<String>,
    },
    /// The issue deleted.
    Delete(Ending),
    /// A comment.
    Comment {
        /// What it says.
        text: String,
        /// Who wrote it, when not the event's actor.
        author: Option<String>,
    },
    /// A dependency added.
    DepAdd {
        /// The target: an issue's uid, or text that names no issue.
        to: String,
        /// Its type; `blocks` when absent or empty.
        kind: Option<String>,
        /// Metadata an import carried.
        meta: Box<Object>,
    },
    /// Dependencies taken back: those to `to`, or an imported one on `key`.
    DepRemove {
        /// The target, as `dep_add` named it.
        to: Option<String>,
        /// The key an imported dependency waits for.
        key: Option<String>,
        /// The type it takes back; every type when absent.
        kind: Option<String>,
    },
    /// Labels added.
    LabelAdd(Vec<String>),
    /// Labels removed.
    LabelRemove(Vec<String>),
    /// The labels, set.
    LabelSet(Vec<String>),
    /// The stand-in for an id with several contents. Only the journal makes one: an event that
    /// names the operation so is [`Op::Unknown`].
    Quarantined,
    /// The stand-in for a record that is not a valid event. Only the journal makes one: an event
    /// that names the operation so is [`Op::Unknown`].
    Invalid,
    /// An operation this bead does not know.
    Unknown(String),
}

/// The member `key` of `data`, taken out of it: a payload is read once, so nothing is copied.
fn take(data: &mut Object, key: &str) -> Option<Value> {
    data.swap_remove(key)
}

/// The text at `key`, taken out.
fn text(data: &mut Object, key: &str) -> Option<String> {
    match take(data, key) {
        Some(Value::Str(text)) => Some(text),
        _ => None,
    }
}

/// The object at `key`, taken out, or an empty one when no object is present.
fn object(data: &mut Object, key: &str) -> Object {
    match take(data, key) {
        Some(Value::Object(members)) => *members,
        _ => Object::new(),
    }
}

/// The items of the array at `key`, taken out, or none when no array is present.
fn items(data: &mut Object, key: &str) -> Vec<Value> {
    match take(data, key) {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    }
}

/// The string members of the array at `key`, or none when no array is present.
fn strings(data: &mut Object, key: &str) -> Vec<String> {
    items(data, key)
        .into_iter()
        .filter_map(|item| match item {
            Value::Str(text) => Some(text),
            _ => None,
        })
        .collect()
}

/// The recognized fields in the `fields` object, in its order, or none when no object is present.
fn fields(data: &mut Object) -> Vec<(Field, Value)> {
    object(data, "fields")
        .into_iter()
        .filter_map(|(name, value)| Some((Field::from_name(&name)?, value)))
        .collect()
}

fn ending(data: &mut Object) -> Ending {
    Ending {
        claim: text(data, "claim"),
        reason: text(data, "reason"),
    }
}

/// An operation an event names, one of the fourteen replay applies, by the name format 1 gives
/// it. Writes name their operation by this, so that a misspelt one does not compile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpName {
    /// `init`: The tracker begins: its prefix and name.
    Init,
    /// `create`: An issue is made, or imported.
    Create,
    /// `set`: Fields of an issue are set or cleared.
    Set,
    /// `claim`: An issue is claimed, or taken over.
    Claim,
    /// `release`: A claim is given up.
    Release,
    /// `close`: An issue is closed, ending the claim it names.
    Close,
    /// `reopen`: A closed issue is opened again.
    Reopen,
    /// `delete`: An issue becomes a tombstone, ending the claim it names.
    Delete,
    /// `comment`: A comment is added to an issue.
    Comment,
    /// `dep_add`: A dependency is added.
    DepAdd,
    /// `dep_remove`: A dependency is taken back.
    DepRemove,
    /// `label_add`: Labels are added.
    LabelAdd,
    /// `label_remove`: Labels are taken away.
    LabelRemove,
    /// `label_set`: An issue's labels are replaced.
    LabelSet,
}

impl OpName {
    /// Every operation, in the order format 1 lists them.
    pub const ALL: [OpName; 14] = [
        OpName::Init,
        OpName::Create,
        OpName::Set,
        OpName::Claim,
        OpName::Release,
        OpName::Close,
        OpName::Reopen,
        OpName::Delete,
        OpName::Comment,
        OpName::DepAdd,
        OpName::DepRemove,
        OpName::LabelAdd,
        OpName::LabelRemove,
        OpName::LabelSet,
    ];

    /// The name an event gives the operation.
    pub const fn as_str(self) -> &'static str {
        match self {
            OpName::Init => "init",
            OpName::Create => "create",
            OpName::Set => "set",
            OpName::Claim => "claim",
            OpName::Release => "release",
            OpName::Close => "close",
            OpName::Reopen => "reopen",
            OpName::Delete => "delete",
            OpName::Comment => "comment",
            OpName::DepAdd => "dep_add",
            OpName::DepRemove => "dep_remove",
            OpName::LabelAdd => "label_add",
            OpName::LabelRemove => "label_remove",
            OpName::LabelSet => "label_set",
        }
    }

    /// The operation `name` names, or `None` for one replay does not apply (a newer bead's, or a
    /// stand-in's `quarantined` and `invalid`).
    pub fn from_name(name: &str) -> Option<OpName> {
        OpName::ALL.into_iter().find(|op| op.as_str() == name)
    }
}

impl Op {
    /// The operation's name, as an event names it: what [`Op::read`] read it from, and for a
    /// stand-in `quarantined` or `invalid`.
    pub fn name(&self) -> &str {
        match self {
            Op::Quarantined => "quarantined",
            Op::Invalid => "invalid",
            Op::Unknown(name) => name,
            known => known.kind().map_or_default(OpName::as_str),
        }
    }

    /// The operation replay applied, or `None` for a stand-in or an operation it does not know.
    pub fn kind(&self) -> Option<OpName> {
        Some(match self {
            Op::Init { .. } => OpName::Init,
            Op::Create(_) => OpName::Create,
            Op::Set(_) => OpName::Set,
            Op::Claim(_) => OpName::Claim,
            Op::Release(_) => OpName::Release,
            Op::Close(_) => OpName::Close,
            Op::Reopen { .. } => OpName::Reopen,
            Op::Delete(_) => OpName::Delete,
            Op::Comment { .. } => OpName::Comment,
            Op::DepAdd { .. } => OpName::DepAdd,
            Op::DepRemove { .. } => OpName::DepRemove,
            Op::LabelAdd(_) => OpName::LabelAdd,
            Op::LabelRemove(_) => OpName::LabelRemove,
            Op::LabelSet(_) => OpName::LabelSet,
            Op::Quarantined | Op::Invalid | Op::Unknown(_) => return None,
        })
    }

    /// The operation `name` asks for, its payload read from `data`, which it takes apart: what
    /// replay keeps of a payload is moved out of it, never copied (roll-lqjj.53.1).
    pub fn read(name: &str, mut data: Object) -> Op {
        let data = &mut data;
        let present = |data: &mut Object, key: &str| {
            take(data, key).filter(|v| *v != Value::Null).map(Box::new)
        };
        let Some(op) = OpName::from_name(name) else {
            // `quarantined` and `invalid` among them: an event cannot ask to be a stand-in, which
            // replay takes as reported when it was loaded.
            return Op::Unknown(name.to_owned());
        };
        match op {
            OpName::Init => Op::Init {
                prefix: present(data, "prefix"),
                name: present(data, "name"),
            },
            OpName::Create => Op::Create(Box::new(Create {
                key: text(data, "key").unwrap_or_default(),
                fields: fields(data),
                labels: strings(data, "labels"),
                extra: object(data, "extra"),
                comments: items(data, "comments")
                    .into_iter()
                    .filter_map(|c| match c {
                        Value::Object(members) => Some(*members),
                        _ => None,
                    })
                    .collect(),
                dangling: items(data, "dangling"),
                imported: data.get("imported").is_some_and(Value::truthy),
                parent: text(data, "parent").as_deref().and_then(EventId::parse),
            })),
            OpName::Set => Op::Set(fields(data)),
            OpName::Claim => Op::Claim(Box::new(Claiming {
                mode: text(data, "mode"),
                takeover: text(data, "takeover"),
                reason: text(data, "reason"),
                generation: data.get("generation").and_then(IntegerField::read),
            })),
            OpName::Release => Op::Release(ending(data)),
            OpName::Close => Op::Close(ending(data)),
            OpName::Reopen => Op::Reopen {
                reason: text(data, "reason"),
            },
            OpName::Delete => Op::Delete(ending(data)),
            OpName::Comment => Op::Comment {
                text: text(data, "text").unwrap_or_default(),
                author: text(data, "author"),
            },
            OpName::DepAdd => Op::DepAdd {
                to: text(data, "to").unwrap_or_default(),
                kind: text(data, "type"),
                meta: Box::new(object(data, "meta")),
            },
            OpName::DepRemove => Op::DepRemove {
                to: text(data, "to"),
                key: text(data, "key"),
                kind: text(data, "type"),
            },
            OpName::LabelAdd => Op::LabelAdd(strings(data, "labels")),
            OpName::LabelRemove => Op::LabelRemove(strings(data, "labels")),
            OpName::LabelSet => Op::LabelSet(strings(data, "labels")),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::json::Object;

    use super::{Op, OpName};

    /// Each operation reads back from its own name, and is what replay reads that name as: the one
    /// list a write names, a payload is checked against and replay applies.
    #[test]
    fn every_operation_is_read_back_from_its_name() {
        for op in OpName::ALL {
            assert_eq!(OpName::from_name(op.as_str()), Some(op));
            let read = Op::read(op.as_str(), Object::new());
            assert_eq!((read.kind(), read.name()), (Some(op), op.as_str()));
        }
        for stand_in in ["quarantined", "invalid", "rename", ""] {
            assert_eq!(OpName::from_name(stand_in), None, "{stand_in:?}");
            assert_eq!(
                Op::read(stand_in, Object::new()).kind(),
                None,
                "{stand_in:?}"
            );
        }
    }
}
