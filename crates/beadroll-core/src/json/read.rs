//! Reads JSON into a [`Value`] with serde_json (roll-lqjj.53): bead's own parser, kept until then
//! for parity with the Python bead it was ported from, is gone with the parity. What a text may
//! hold is serde_json's strict JSON: UTF-8 throughout, a lone surrogate (escaped or as raw bytes)
//! refused, `NaN`, `Infinity` and a number beyond a float refused, at most [`MAX_DEPTH`] levels of
//! arrays and objects. An integer that fits a `u64` or an `i64` is exact; one beyond them, which
//! bead never writes, arrives as a float. A repeated key keeps its last value at its first place.

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

use super::{Int, Object, Value};

/// Maximum array and object nesting accepted by readers: serde_json's recursion limit, well above
/// the 100 levels an event may use (`MAX_NESTING`). An event written with more was never valid.
pub const MAX_DEPTH: usize = 127;

/// A text read as JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    /// The value.
    pub value: Value,
}

/// Why a text cannot be read as JSON: serde_json's diagnostic, with its line and column.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(String);

/// Reads `bytes` as one JSON document, whitespace around it allowed, nothing else.
pub fn parse(bytes: &[u8]) -> Result<Parsed, ParseError> {
    serde_json::from_slice(bytes)
        .map(|value| Parsed { value })
        .map_err(|error| ParseError(error.to_string()))
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

/// Builds a [`Value`] from whatever the deserializer finds, each object once, by key as written.
struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, flag: bool) -> Result<Value, E> {
        Ok(Value::Bool(flag))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Int(Int::from(value)))
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
        match i64::try_from(value) {
            Ok(value) => Ok(Value::Int(Int::from(value))),
            // beyond i64: its digits, exact, as the writer prints them
            Err(_) => Int::parse(&value.to_string())
                .map(Value::Int)
                .ok_or_else(|| E::custom("an integer beyond what bead keeps")),
        }
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
        if value.is_finite() {
            Ok(Value::Float(value))
        } else {
            Err(E::custom("a number beyond a float"))
        }
    }

    fn visit_str<E>(self, text: &str) -> Result<Value, E> {
        Ok(Value::Str(text.to_owned()))
    }

    fn visit_string<E>(self, text: String) -> Result<Value, E> {
        Ok(Value::Str(text))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Value, A::Error> {
        let mut out = Vec::with_capacity(items.size_hint().unwrap_or(0));
        while let Some(item) = items.next_element()? {
            out.push(item);
        }
        Ok(Value::Array(out))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut members: A) -> Result<Value, A::Error> {
        let mut out = Object::with_capacity(members.size_hint().unwrap_or(0));
        while let Some((key, value)) = members.next_entry::<String, Value>()? {
            // a repeated key keeps its first place and takes the last value
            out.insert(key, value);
        }
        Ok(Value::Object(Box::new(out)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Format, write};

    fn message(text: &[u8]) -> String {
        parse(text).map_or_else(|error| error.to_string(), |_| "parsed".to_owned())
    }

    fn compact(text: &[u8]) -> String {
        write(&parse(text).unwrap().value, Format::COMPACT).unwrap()
    }

    #[test]
    fn objects_keep_the_order_written_and_a_repeated_key_its_first_place_and_last_value() {
        assert_eq!(
            compact(b"{\"b\":1,\"a\":2,\"c\":3}"),
            "{\"b\":1,\"a\":2,\"c\":3}"
        );
        assert_eq!(compact(b"{\"a\":1,\"b\":2,\"a\":3}"), "{\"a\":3,\"b\":2}");
        assert_eq!(
            compact(b" [ 1 , {} , [] , null , true ] \n"),
            "[1,{},[],null,true]"
        );
    }

    #[test]
    fn integers_are_exact_within_a_u64_and_floats_beyond_them() {
        assert_eq!(parse(b"-0").unwrap().value, Value::Float(-0.0));
        assert_eq!(parse(b"0").unwrap().value, Value::from(0));
        assert_eq!(
            parse(b"-9223372036854775808").unwrap().value,
            Value::from(i64::MIN)
        );
        assert_eq!(compact(b"[18446744073709551615]"), "[18446744073709551615]");
        // beyond u64 and below i64: a float, and never an error, since bead writes neither
        assert_eq!(
            parse(b"18446744073709551616").unwrap().value,
            Value::Float(18_446_744_073_709_551_616.0)
        );
        assert_eq!(
            parse(b"-9223372036854775809").unwrap().value,
            Value::Float(-9_223_372_036_854_775_809.0)
        );
        assert_eq!(
            compact(b"[1.0, 1e-05, 1E+16, 0.1]"),
            "[1.0,1e-05,1e+16,0.1]"
        );
    }

    #[test]
    fn numbers_json_has_no_room_for_are_refused() {
        for text in ["NaN", "[Infinity]", "[-Infinity]", "1e400", "[-1e400]"] {
            assert_ne!(message(text.as_bytes()), "parsed", "{text}");
        }
        assert_eq!(message(b"1e400"), "number out of range at line 1 column 5");
        assert_eq!(parse(b"1e-400").unwrap().value, Value::Float(0.0));
    }

    #[test]
    fn nesting_stops_at_the_ceiling_and_strings_hold_any_bracket() {
        let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(parse(deep(MAX_DEPTH).as_bytes()).is_ok());
        assert_eq!(
            message(deep(MAX_DEPTH + 1).as_bytes()),
            "recursion limit exceeded at line 1 column 128"
        );
        let objects = |n: usize| format!("{}1{}", "{\"a\":".repeat(n), "}".repeat(n));
        assert!(parse(objects(MAX_DEPTH).as_bytes()).is_ok());
        assert!(parse(objects(MAX_DEPTH + 1).as_bytes()).is_err());
        assert!(parse(format!("[\"{}\"]", "[{".repeat(600)).as_bytes()).is_ok());
    }

    #[test]
    fn texts_are_utf8_without_surrogates_and_one_document_each() {
        assert_eq!(
            message(b"\"\xff\""),
            "invalid unicode code point at line 1 column 2"
        );
        // a lone surrogate, escaped or as the raw bytes ED A0 80, is not text
        assert_eq!(
            message(b"\"a\\ud800b\""),
            "unexpected end of hex escape at line 1 column 9"
        );
        assert_eq!(
            message(b"\"\\udc00\""),
            "lone leading surrogate in hex escape at line 1 column 7"
        );
        assert_eq!(
            message(b"\"a\xed\xa0\x80b\""),
            "invalid unicode code point at line 1 column 3"
        );
        assert_eq!(
            parse(b"\"\\ud83d\\ude00\"").unwrap().value,
            Value::from("\u{1f600}")
        );
        assert_eq!(
            message(b"\xef\xbb\xbf[]"),
            "expected value at line 1 column 1"
        );
        assert_eq!(
            message(b"\"a\tb\""),
            "control character (\\u0000-\\u001F) found while parsing a string at line 1 column 3"
        );
        assert_eq!(message(b"[] x"), "trailing characters at line 1 column 4");
        assert_eq!(message(b""), "EOF while parsing a value at line 1 column 0");
        assert_eq!(message(b"[1,]"), "trailing comma at line 1 column 4");
        assert_eq!(message(b"\n\n  ]"), "expected value at line 3 column 3");
    }
}
