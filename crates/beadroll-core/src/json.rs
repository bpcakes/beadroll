//! JSON as bead's Python implementation reads and writes it (`docs/RUST-PORT.md` §2.3a), so that a
//! Rust and a Python machine sharing a tracker never read a line differently or write bytes the
//! other would rewrite.
//!
//! - [`parse()`] reads bytes as `loads_strict` does: UTF-8 with lone surrogates kept (each reads as one
//!   U+FFFD and is reported), no deeper than [`MAX_DEPTH`], with `NaN`, `Infinity` and numbers too
//!   large for a float refused, integers exact within [`MAX_DIGITS`] digits, and a repeated key
//!   keeping its last value in its first place.
//! - [`write()`] prints a value in one of Python's `json.dumps` dialects ([`Format`]), floats as
//!   `repr()` prints them.
//! - [`Surrogates`] keeps the lone surrogates Python reads a file's escapes as, through a parse and
//!   a write, for the files bead must hand back as Python would.

mod int;
mod parse;
mod surrogates;
mod write;

use indexmap::IndexMap;

pub use int::{Int, MAX_DIGITS};
pub use parse::{MAX_DEPTH, ParseError, Parsed, nesting, parse};
pub use surrogates::{Surrogates, lone_surrogate_escapes};
pub use write::{Format, Style, WriteError, float_repr, write};

/// A JSON object: its members in the order Python's dict keeps them.
pub type Object = IndexMap<String, Value>;

/// A JSON value as Python holds one after `json.loads`.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `null`, Python's `None`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer token, exact.
    Int(Int),
    /// A number with a fraction or an exponent: always finite.
    Float(f64),
    /// A string.
    Str(String),
    /// An array.
    Array(Vec<Value>),
    /// An object, boxed: a value is as large as its largest kind, and objects are a few of the
    /// values a journal and its state hold (roll-dih8.11).
    Object(Box<Object>),
}

/// How many members an object may have and still be searched in order: comparing a few keys is
/// quicker than hashing one, and an event and most of what it holds are that small.
const SCAN: usize = 12;

/// The member `key` of `members`, as `IndexMap::get` finds it (keys are unique): a small object
/// is searched in order, a larger one by hash.
pub(crate) fn member<'a>(members: &'a Object, key: &str) -> Option<&'a Value> {
    if members.len() <= SCAN {
        members
            .iter()
            .find(|(name, _)| name.as_str() == key)
            .map(|(_, value)| value)
    } else {
        members.get(key)
    }
}

impl Value {
    /// The member `key`, if this is an object that has it: Python's `value.get(key)` on a dict.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => member(members, key),
            _ => None,
        }
    }

    /// The text, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The members, if this is an object.
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(members) => Some(members),
            _ => None,
        }
    }

    /// The items, if this is an array.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The flag, if this is `true` or `false`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    /// The integer, if this is one that fits an `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(int) => int.as_i64(),
            _ => None,
        }
    }

    /// Python's truth value: `None`, `False`, zero, and empty strings, arrays and objects are false.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(value) => *value,
            Value::Int(value) => value.as_i64() != Some(0),
            Value::Float(value) => *value != 0.0,
            Value::Str(text) => !text.is_empty(),
            Value::Array(items) => !items.is_empty(),
            Value::Object(members) => !members.is_empty(),
        }
    }

    /// Whether arrays and objects nest more than `limit` deep: Python's `too_deep`, without recursion.
    pub fn deeper_than(&self, limit: usize) -> bool {
        let mut stack = vec![(self, 1usize)];
        while let Some((value, depth)) = stack.pop() {
            let children: Box<dyn Iterator<Item = &Value>> = match value {
                Value::Array(items) => Box::new(items.iter()),
                Value::Object(members) => Box::new(members.values()),
                _ => continue,
            };
            if depth > limit {
                return true;
            }
            stack.extend(children.map(|child| (child, depth + 1)));
        }
        false
    }
}

/// What indexing answers for a member or item that is not there.
static NULL: Value = Value::Null;

impl std::ops::Index<&str> for Value {
    type Output = Value;

    /// The member `key`, or `null` when this is not an object or has no such member.
    fn index(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&NULL)
    }
}

impl std::ops::Index<usize> for Value {
    type Output = Value;

    /// The item at `index`, or `null` when this is not an array or is shorter.
    fn index(&self, index: usize) -> &Value {
        self.as_array()
            .and_then(|items| items.get(index))
            .unwrap_or(&NULL)
    }
}

/// `text` read as JSON, as `json.loads` reads a string (a lone surrogate reads as U+FFFD).
pub fn from_text(text: &str) -> Result<Value, ParseError> {
    parse(text.as_bytes()).map(|parsed| parsed.value)
}

impl From<&str> for Value {
    fn from(text: &str) -> Value {
        Value::Str(text.to_owned())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Value {
        Value::Str(text)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Value {
        Value::Int(Int::from(value))
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Value {
        Value::Bool(value)
    }
}

impl From<Object> for Value {
    fn from(members: Object) -> Value {
        Value::Object(Box::new(members))
    }
}

impl From<Vec<Value>> for Value {
    fn from(items: Vec<Value>) -> Value {
        Value::Array(items)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Value {
        value.map_or(Value::Null, Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truth_follows_python() {
        let falsy = [
            Value::Null,
            Value::Bool(false),
            Value::from(0),
            Value::Float(0.0),
            Value::Float(-0.0),
            Value::from(""),
            Value::Array(vec![]),
            Value::from(Object::new()),
        ];
        assert!(falsy.iter().all(|v| !v.truthy()));
        let truthy = [
            Value::Bool(true),
            Value::from(-1),
            Value::Float(0.5),
            Value::from(" "),
            Value::Array(vec![Value::Null]),
        ];
        assert!(truthy.iter().all(Value::truthy));
    }

    #[test]
    fn depth_counts_containers_only() {
        let mut value = Value::from(1);
        for _ in 0..3 {
            value = Value::Array(vec![value]);
        }
        assert!(!value.deeper_than(3));
        assert!(value.deeper_than(2));
        assert!(!Value::from("x").deeper_than(0));
    }
}
