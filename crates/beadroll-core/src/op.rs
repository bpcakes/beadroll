//! An event's typed operation. Payloads are read only after
//! validation; invalid events are never applied. A member of the wrong type therefore reads as
//! absent, and missing or falsy members use operation-specific defaults.

use std::cmp::Ordering;
use std::fmt;

use crate::event::Field;
use crate::json::{Int, Object, Value};

/// An integer field read from a format-1 event, retaining a legacy boolean representation.
/// Replay accepts booleans in integer fields of old histories (`docs/REVIEWING.md` §2).
/// Comparison uses their numeric values (`true` equals 1, `false` equals 0), while JSON keeps
/// the original type and display keeps the established `True`/`False` spelling (`[PTrue]`).
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
        match self {
            IntegerField::Bool(true) => f.write_str("True"),
            IntegerField::Bool(false) => f.write_str("False"),
            IntegerField::Int(int) => write!(f, "{int}"),
        }
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
}

/// What a claim carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Claiming {
    /// `offline` for a provisional claim; `online` when absent or empty.
    pub mode: Option<String>,
    /// The claim it takes over.
    pub takeover: Option<String>,
    /// Why, for a takeover.
    pub reason: Option<String>,
    /// The generation its writer saw.
    pub generation: Option<IntegerField>,
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
    /// The issue reopened.
    Reopen,
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
    /// The stand-in for an id with several contents (or an event that names the operation so).
    Quarantined,
    /// The stand-in for a record that is not a valid event (or an event that names the operation
    /// so).
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

impl Op {
    /// The operation's name, as an event names it: what [`Op::read`] read it from.
    pub fn name(&self) -> &str {
        match self {
            Op::Init { .. } => "init",
            Op::Create(_) => "create",
            Op::Set(_) => "set",
            Op::Claim(_) => "claim",
            Op::Release(_) => "release",
            Op::Close(_) => "close",
            Op::Reopen => "reopen",
            Op::Delete(_) => "delete",
            Op::Comment { .. } => "comment",
            Op::DepAdd { .. } => "dep_add",
            Op::DepRemove { .. } => "dep_remove",
            Op::LabelAdd(_) => "label_add",
            Op::LabelRemove(_) => "label_remove",
            Op::LabelSet(_) => "label_set",
            Op::Quarantined => "quarantined",
            Op::Invalid => "invalid",
            Op::Unknown(name) => name,
        }
    }

    /// The operation `name` asks for, its payload read from `data`, which it takes apart: what
    /// replay keeps of a payload is moved out of it, never copied (roll-lqjj.53.1).
    pub fn read(name: &str, mut data: Object) -> Op {
        let data = &mut data;
        let present = |data: &mut Object, key: &str| {
            take(data, key).filter(|v| *v != Value::Null).map(Box::new)
        };
        match name {
            "init" => Op::Init {
                prefix: present(data, "prefix"),
                name: present(data, "name"),
            },
            "create" => Op::Create(Box::new(Create {
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
            })),
            "set" => Op::Set(fields(data)),
            "claim" => Op::Claim(Box::new(Claiming {
                mode: text(data, "mode"),
                takeover: text(data, "takeover"),
                reason: text(data, "reason"),
                generation: data.get("generation").and_then(IntegerField::read),
            })),
            "release" => Op::Release(ending(data)),
            "close" => Op::Close(ending(data)),
            "reopen" => Op::Reopen,
            "delete" => Op::Delete(ending(data)),
            "comment" => Op::Comment {
                text: text(data, "text").unwrap_or_default(),
                author: text(data, "author"),
            },
            "dep_add" => Op::DepAdd {
                to: text(data, "to").unwrap_or_default(),
                kind: text(data, "type"),
                meta: Box::new(object(data, "meta")),
            },
            "dep_remove" => Op::DepRemove {
                to: text(data, "to"),
                key: text(data, "key"),
                kind: text(data, "type"),
            },
            "label_add" => Op::LabelAdd(strings(data, "labels")),
            "label_remove" => Op::LabelRemove(strings(data, "labels")),
            "label_set" => Op::LabelSet(strings(data, "labels")),
            "quarantined" => Op::Quarantined,
            "invalid" => Op::Invalid,
            other => Op::Unknown(other.to_owned()),
        }
    }
}
