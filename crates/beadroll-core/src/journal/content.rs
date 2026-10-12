//! What a record holds, as one digest (roll-dih8.11): two records hold the same content exactly
//! when their digests are equal, as Python's `canonical` (sorted keys, compact) tells contents
//! apart: object members in any order, floats by their bits (`-0.0` is not `0.0`), the rest
//! exactly. A content is encoded so that no two contents encode alike: each value's kind named,
//! every string and list given its length, an integer its digits and an end, each object's members
//! taken in the order of their keys, without recursion, so that a value of any depth has one. The
//! encoding is then hashed twice with SipHash-1-3 under keys drawn for the journal, 128 bits that
//! nobody without the keys can make two contents share; the keys never leave the process, and only
//! contents one journal holds are compared.

use std::hash::{BuildHasher as _, RandomState};
use std::io::Write as _;

use crate::json::Value;

/// A record's content: equal for two records of one journal exactly when they hold the same
/// content.
pub(crate) type Digest = [u64; 2];

/// What is left to encode: a value, or an object member, its key before its value.
enum Step<'a> {
    Value(&'a Value),
    Member(&'a str, &'a Value),
}

/// A journal's digests: its keys, shared by its clones, and room to encode contents in, kept from
/// one record to the next.
#[derive(Clone, Debug, Default)]
pub(crate) struct Digester {
    keys: [RandomState; 2],
    bytes: Vec<u8>,
}

impl Digester {
    /// The digest of `value`'s content.
    pub(crate) fn digest(&mut self, value: &Value) -> Digest {
        let out = &mut self.bytes;
        out.clear();
        let mut stack = vec![Step::Value(value)];
        while let Some(step) = stack.pop() {
            let value = match step {
                Step::Member(key, value) => {
                    text(out, b'k', key);
                    stack.push(Step::Value(value));
                    continue;
                }
                Step::Value(value) => value,
            };
            match value {
                Value::Null => out.push(b'n'),
                Value::Bool(true) => out.push(b't'),
                Value::Bool(false) => out.push(b'f'),
                Value::Int(n) => {
                    // its digits and an end, written where the encoding is
                    out.push(b'i');
                    let _ = write!(out, "{n};");
                }
                Value::Float(x) => {
                    out.push(b'd');
                    out.extend_from_slice(&x.to_bits().to_be_bytes());
                }
                Value::Str(s) => text(out, b's', s),
                Value::Array(items) => {
                    count(out, b'a', items.len());
                    stack.extend(items.iter().rev().map(Step::Value));
                }
                Value::Object(members) => {
                    count(out, b'o', members.len());
                    let mark = stack.len();
                    stack.extend(members.iter().map(|(key, value)| Step::Member(key, value)));
                    // in descending key order, so that they are taken in ascending order
                    let members = stack.get_mut(mark..).unwrap_or_default();
                    members.sort_unstable_by(|a, b| key(b).cmp(key(a)));
                }
            }
        }
        let bytes = out.as_slice();
        [self.keys[0].hash_one(bytes), self.keys[1].hash_one(bytes)]
    }
}

/// A member's key.
fn key<'a>(step: &Step<'a>) -> &'a str {
    match step {
        Step::Member(key, _) => key,
        Step::Value(_) => "",
    }
}

/// A kind and a length, in seven bits a byte, the last byte's high bit clear.
fn count(out: &mut Vec<u8>, kind: u8, len: usize) {
    out.push(kind);
    let mut rest = len;
    while rest >= 0x80 {
        out.push((rest & 0x7f) as u8 | 0x80);
        rest >>= 7;
    }
    out.push(rest as u8);
}

/// A kind and a text, with its length.
fn text(out: &mut Vec<u8>, kind: u8, text: &str) {
    count(out, kind, text.len());
    out.extend_from_slice(text.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn digest(text: &str) -> Digest {
        thread_local! {
            static DIGESTER: std::cell::RefCell<Digester> = Default::default();
        }
        let value = parse(text.as_bytes()).unwrap().value;
        DIGESTER.with(|digester| digester.borrow_mut().digest(&value))
    }

    #[test]
    fn contents_are_equal_whatever_the_order_of_their_members() {
        assert_eq!(
            digest(r#"{"a": 1, "b": {"x": [1, 2], "y": null}}"#),
            digest(r#"{"b":{"y":null,"x":[1,2]},"a":1}"#)
        );
        // a repeated key keeps its last value at its first key's place, as Python reads it
        assert_eq!(digest(r#"{"a": 1, "a": 2}"#), digest(r#"{"a": 2}"#));
    }

    #[test]
    fn contents_that_differ_have_different_digests() {
        let distinct = [
            "null",
            "true",
            "false",
            "0",
            "1",
            "1.0",
            "-0.0",
            "0.0",
            r#""""#,
            r#""1""#,
            "[]",
            "{}",
            "[[]]",
            "[{}]",
            r#"{"": null}"#,
            r#"{"a": "b"}"#,
            r#"{"ab": ""}"#,
            r#"["a", "b"]"#,
            r#"["ab"]"#,
            r#"{"a": ["b"]}"#,
            r#"[{"a": "b"}]"#,
            r#"{"a": 1, "b": 2}"#,
            r#"{"a": 2, "b": 1}"#,
            "100000000000000000000000",
        ];
        for (i, a) in distinct.iter().enumerate() {
            for b in &distinct[i + 1..] {
                assert_ne!(digest(a), digest(b), "{a} and {b}");
            }
        }
    }

    #[test]
    fn many_members_sort_as_few_do() {
        let members: Vec<String> = (0..40)
            .map(|n| format!("\"k{}\": {n}", (n * 7) % 40))
            .collect();
        let mut sorted = members.clone();
        sorted.sort();
        let forward = format!("{{{}}}", members.join(", "));
        let backward = format!(
            "{{{}}}",
            sorted.iter().rev().cloned().collect::<Vec<_>>().join(", ")
        );
        assert_eq!(digest(&forward), digest(&backward));
    }

    #[test]
    fn a_value_of_any_depth_has_a_digest() {
        let deep = format!("{}{}", "[".repeat(500), "]".repeat(500));
        let deeper = format!("{}{}", "[".repeat(501), "]".repeat(501));
        assert_ne!(digest(&deep), digest(&deeper));
    }
}
