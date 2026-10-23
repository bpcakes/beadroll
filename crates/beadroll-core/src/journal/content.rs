//! A record's content digest (roll-dih8.11): two SipHash-1-3 hashes, under keys drawn at random
//! for each journal, of the bytes of its line, whitespace around it left out; of the line bead
//! writes of it, when the record was built in memory (roll-lqjj.53.1). Two records of one id with
//! the same digest hold the same content, which is to say the same text: bead writes every record
//! once, compact and with its members in one order, and copies it byte for byte, so two lines of
//! one content are the same bytes. The keys stay in-process, and only contents within the same
//! journal are compared.

use std::hash::{BuildHasher as _, Hash as _, Hasher as _, RandomState};

use crate::json::{Format, Value, write};

/// A record's content: equal for two records of one journal exactly when they hold the same
/// content.
pub(crate) type Digest = [u64; 2];

/// A journal's digests: its keys, shared by its clones.
#[derive(Clone, Debug, Default)]
pub(crate) struct Digester {
    keys: [RandomState; 2],
}

impl Digester {
    /// The digest of the content whose line is `bytes`, whitespace around it left out.
    pub(crate) fn digest(&self, bytes: &[u8]) -> Digest {
        let bytes = crate::text::strip_bytes(bytes);
        [self.keys[0].hash_one(bytes), self.keys[1].hash_one(bytes)]
    }

    /// The digest of `value`'s content: that of the line bead writes of it. Only a value built in
    /// memory fails to print (an integer too long, nesting too deep); an iterative encoding
    /// keeps those contents distinct without asking the call stack to follow their depth.
    pub(crate) fn digest_of(&self, value: &Value) -> Digest {
        match write(value, Format::COMPACT) {
            Ok(line) => self.digest(line.as_bytes()),
            Err(_) => self.keys.each_ref().map(|key| fallback(key, value)),
        }
    }
}

/// An unprintable value's typed, length-delimited contents, in written member order. A separate
/// domain keeps this encoding apart from serialized lines; no recursive formatting or hashing
/// of a container is used, even when a caller built it deeper than JSON permits.
fn fallback(key: &RandomState, value: &Value) -> u64 {
    let mut hash = key.build_hasher();
    "unprintable event".hash(&mut hash);
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        std::mem::discriminant(value).hash(&mut hash);
        match value {
            Value::Null => {}
            Value::Bool(value) => value.hash(&mut hash),
            Value::Int(value) => value.to_string().hash(&mut hash),
            Value::Float(value) => value.to_bits().hash(&mut hash),
            Value::Str(value) => value.hash(&mut hash),
            Value::Array(items) => {
                items.len().hash(&mut hash);
                pending.extend(items.iter().rev());
            }
            Value::Object(members) => {
                members.len().hash(&mut hash);
                for name in members.keys() {
                    name.hash(&mut hash);
                }
                pending.extend(members.values().rev());
            }
        }
    }
    hash.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn digester() -> Digester {
        Digester::default()
    }

    #[test]
    fn a_line_and_the_value_read_from_it_have_one_digest() {
        let digester = digester();
        for line in [
            r#"{"a": 1, "b": {"x": [1, 2], "y": null}}"#,
            r#"{"b":{"y":null,"x":[1,2]},"a":1}"#,
            r#"[1.0, -0.0, "téxt", 18446744073709551615]"#,
        ] {
            let value = parse(line.as_bytes()).unwrap().value;
            let written = write(&value, Format::COMPACT).unwrap();
            assert_eq!(
                digester.digest(written.as_bytes()),
                digester.digest_of(&value)
            );
            assert_eq!(
                digester.digest(format!("  {written}\r\n").as_bytes()),
                digester.digest_of(&value)
            );
        }
    }

    #[test]
    fn contents_written_otherwise_have_different_digests() {
        let digester = digester();
        let distinct = [
            "null",
            "true",
            "0",
            "1.0",
            "-0.0",
            r#""""#,
            "[]",
            "{}",
            r#"{"a": 1, "b": 2}"#,
            r#"{"b": 2, "a": 1}"#,
            r#"{"a":1,"b":2}"#,
        ];
        for (i, a) in distinct.iter().enumerate() {
            for b in &distinct[i + 1..] {
                assert_ne!(
                    digester.digest(a.as_bytes()),
                    digester.digest(b.as_bytes()),
                    "{a} and {b}"
                );
            }
        }
    }

    #[test]
    fn journals_draw_their_own_keys() {
        let line = br#"{"a": 1}"#;
        let one = digester();
        let shared = one.clone();
        assert_eq!(one.digest(line), shared.digest(line));
        assert_ne!(one.digest(line), digester().digest(line));
    }

    #[test]
    fn unprintable_contents_keep_types_lengths_and_member_order() {
        let digester = digester();
        let mut digests = Vec::new();
        for text in [
            "null",
            "false",
            "0",
            "0.0",
            "-0.0",
            "[]",
            "{}",
            r#"["ab","c"]"#,
            r#"["a","bc"]"#,
            r#"{"a":1,"b":2}"#,
            r#"{"b":2,"a":1}"#,
        ] {
            let value = Value::Array(vec![
                Value::Float(f64::INFINITY),
                parse(text.as_bytes()).unwrap().value,
            ]);
            let digest = digester.digest_of(&value);
            assert!(!digests.contains(&digest), "{text}");
            assert_eq!(digest, digester.digest_of(&value));
            digests.push(digest);
        }
    }

    #[test]
    fn a_value_of_any_depth_has_a_digest() {
        // deeper than any text reads, as a value built in memory may be
        let nested = |depth: usize| {
            (0..depth).fold(Value::Array(Vec::new()), |inner, _| {
                Value::Array(vec![inner])
            })
        };
        let digester = digester();
        let mut one = nested(100_000);
        let mut two = nested(100_001);
        let different = digester.digest_of(&one) != digester.digest_of(&two);
        // Dropping the test inputs recursively would test Value's destructor, not the digest.
        for value in [&mut one, &mut two] {
            while let Value::Array(mut items) = std::mem::replace(value, Value::Null) {
                *value = items.pop().unwrap_or(Value::Null);
            }
        }
        assert!(different);
    }
}
