//! The tracked configuration's known fields read with their types, preserving its public map.

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};

use crate::json::{self, Object, ParseError, Value};

/// Reads the known text and boolean fields directly, moving them into the configuration's
/// public map. Unknown fields stay generic and retain their written positions. A type mismatch
/// falls back to the generic reader so final duplicate values and established validation
/// diagnostics, including a nonobject root, remain the caller's decision.
pub fn read(bytes: &[u8]) -> Result<Value, ParseError> {
    match json::deserialize::<Configuration>(bytes) {
        Ok(config) => Ok(Value::from(config.0)),
        Err(_) => json::parse(bytes).map(|parsed| parsed.value),
    }
}

struct Configuration(Object);

impl<'de> Deserialize<'de> for Configuration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Read;
        impl<'de> Visitor<'de> for Read {
            type Value = Configuration;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a tracked configuration")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Configuration, A::Error> {
                let mut members = Object::new();
                while let Some(name) = map.next_key::<String>()? {
                    let value = match name.as_str() {
                        "workspace" | "remote" | "ref" => Value::Str(map.next_value::<String>()?),
                        "where" => Value::Bool(map.next_value::<bool>()?),
                        _ => map.next_value::<Value>()?,
                    };
                    members.insert(name, value);
                }
                Ok(Configuration(members))
            }
        }
        deserializer.deserialize_map(Read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Format, write};

    #[test]
    fn typed_configuration_keeps_the_public_value_and_diagnostics() {
        for text in [
            r#"{"workspace":"w","unknown":[1],"where":true,"remote":"r","ref":"refs/beads/journal"}"#,
            r#"{"where":null,"unknown":1,"where":false,"workspace":{},"workspace":"w"}"#,
            r#"{"remote":true,"where":"bad"}"#,
            "[]",
            "null",
            "{",
            "{} trailing",
        ] {
            let typed = read(text.as_bytes()).map(|value| write(&value, Format::COMPACT).unwrap());
            let generic = json::parse(text.as_bytes())
                .map(|parsed| write(&parsed.value, Format::COMPACT).unwrap());
            assert_eq!(typed, generic, "{text}");
        }
    }
}
