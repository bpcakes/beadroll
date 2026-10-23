//! Store metadata with published counts read as integers, retaining every member for rewrites.

use std::fmt;

use indexmap::IndexMap;
use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};

use crate::json::{self, Int, Object, ParseError, Value};
use crate::text::parse_integer;

/// The ordered fields of `meta.json`. Ordinary fields stay generic; the large published map
/// holds counts directly rather than a JSON value for every integer.
#[derive(Clone, Debug, Default)]
pub struct Metadata {
    members: IndexMap<String, Member>,
}

#[derive(Clone, Debug)]
enum Member {
    Published(Counts),
    Ordinary(Value),
}

/// Published file names and their counts. Legacy noninteger values are preserved for rewrites
/// and coerced only when a caller asks for that file's count.
#[derive(Clone, Debug, Default)]
pub struct Counts(IndexMap<String, Count>);

#[derive(Clone, Debug)]
enum Count {
    Integer(Int),
    Legacy(Box<Value>),
}

impl Metadata {
    /// Reads metadata. A nonobject returns `None`; malformed JSON retains the generic reader's
    /// diagnostic. Legacy shapes use that reader, including last-wins duplicate members.
    pub fn read(bytes: &[u8]) -> Result<Option<Self>, ParseError> {
        match json::deserialize(bytes) {
            Ok(meta) => Ok(Some(meta)),
            Err(_) => Ok(match json::parse(bytes)?.value {
                Value::Object(members) => Some(Self::from_object(*members)),
                _ => None,
            }),
        }
    }

    /// Takes ownership of an existing metadata object, without changing its order or values.
    pub fn from_object(members: Object) -> Self {
        Self {
            members: members
                .into_iter()
                .map(|(name, value)| {
                    let member = match (name.as_str(), value) {
                        ("published", Value::Object(counts)) => Member::Published(Counts(
                            counts
                                .into_iter()
                                .map(|(name, value)| {
                                    (
                                        name,
                                        match value {
                                            Value::Int(n) => Count::Integer(n),
                                            value => Count::Legacy(Box::new(value)),
                                        },
                                    )
                                })
                                .collect(),
                        )),
                        (_, value) => Member::Ordinary(value),
                    };
                    (name, member)
                })
                .collect(),
        }
    }

    /// An ordinary field. An object-valued `published` field is available through [`Self::published`].
    pub fn value(&self, name: &str) -> Option<&Value> {
        match self.members.get(name) {
            Some(Member::Ordinary(value)) => Some(value),
            _ => None,
        }
    }

    /// The published map, if that field is an object.
    pub fn published(&self) -> Option<&Counts> {
        match self.members.get("published") {
            Some(Member::Published(counts)) => Some(counts),
            _ => None,
        }
    }

    /// Restores the generic public representation for a rewrite or a compatibility caller.
    pub fn into_object(self) -> Object {
        self.members
            .into_iter()
            .map(|(name, member)| {
                let value = match member {
                    Member::Ordinary(value) => value,
                    Member::Published(counts) => Value::from(
                        counts
                            .0
                            .into_iter()
                            .map(|(name, count)| {
                                (
                                    name,
                                    match count {
                                        Count::Integer(n) => Value::Int(n),
                                        Count::Legacy(value) => *value,
                                    },
                                )
                            })
                            .collect::<Object>(),
                    ),
                };
                (name, value)
            })
            .collect()
    }
}

impl Counts {
    /// Whether a file was published; attempt files need only membership, not numeric coercion.
    pub fn contains_key(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    /// A file's count, or zero when absent. Falsy legacy values are zero, true is one, finite
    /// floats truncate toward zero, and strings use integer parsing. Other values return `None`.
    pub fn count(&self, name: &str) -> Option<Int> {
        match self.0.get(name) {
            None => Some(Int::from(0)),
            Some(Count::Integer(n)) => Some(n.clone()),
            Some(Count::Legacy(value)) => match &**value {
                value if !value.truthy() => Some(Int::from(0)),
                Value::Bool(true) => Some(Int::from(1)),
                Value::Float(f) => Int::from_f64(*f),
                Value::Str(text) => parse_integer(text),
                _ => None,
            },
        }
    }
}

impl<'de> Deserialize<'de> for Metadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Read;
        impl<'de> Visitor<'de> for Read {
            type Value = Metadata;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a metadata object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Metadata, A::Error> {
                let mut members = IndexMap::new();
                while let Some(name) = map.next_key::<String>()? {
                    let member = if name == "published" {
                        Member::Published(map.next_value()?)
                    } else {
                        Member::Ordinary(map.next_value()?)
                    };
                    members.insert(name, member);
                }
                Ok(Metadata { members })
            }
        }
        deserializer.deserialize_map(Read)
    }
}

impl<'de> Deserialize<'de> for Counts {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Read;
        impl<'de> Visitor<'de> for Read {
            type Value = Counts;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("published counts")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Counts, A::Error> {
                let mut counts = IndexMap::new();
                while let Some((name, count)) = map.next_entry()? {
                    counts.insert(name, count);
                }
                Ok(Counts(counts))
            }
        }
        deserializer.deserialize_map(Read)
    }
}

impl<'de> Deserialize<'de> for Count {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Read;
        impl Visitor<'_> for Read {
            type Value = Count;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an integer count")
            }
            fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<Count, E> {
                Ok(Count::Integer(Int::from(n)))
            }
            fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<Count, E> {
                match i64::try_from(n) {
                    Ok(n) => Ok(Count::Integer(Int::from(n))),
                    Err(_) => Int::parse(&n.to_string())
                        .map(Count::Integer)
                        .ok_or_else(|| E::custom("an integer beyond what bead keeps")),
                }
            }
        }
        deserializer.deserialize_any(Read)
    }
}

#[cfg(test)]
mod tests;
