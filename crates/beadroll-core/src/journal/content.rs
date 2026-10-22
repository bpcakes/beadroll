//! A record's content digest (roll-dih8.11): two SipHash-1-3 hashes, under keys drawn at random
//! for each journal, of the bytes of its line, whitespace around it left out; of the line bead
//! writes of it, when the record was built in memory (roll-lqjj.53.1). Two records of one id with
//! the same digest hold the same content, which is to say the same text: bead writes every record
//! once, compact and with its members in one order, and copies it byte for byte, so two lines of
//! one content are the same bytes. The keys stay in-process, and only contents within the same
//! journal are compared.

use std::hash::{BuildHasher as _, RandomState};

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
    /// memory fails to print (an integer too long, nesting too deep); its debug form still tells
    /// contents apart.
    pub(crate) fn digest_of(&self, value: &Value) -> Digest {
        match write(value, Format::COMPACT) {
            Ok(line) => self.digest(line.as_bytes()),
            Err(_) => self.digest(format!("{value:?}").as_bytes()),
        }
    }
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
    fn a_value_of_any_depth_has_a_digest() {
        // deeper than any text reads, as a value built in memory may be
        let nested = |depth: usize| {
            (0..depth).fold(Value::Array(Vec::new()), |inner, _| {
                Value::Array(vec![inner])
            })
        };
        let digester = digester();
        assert_ne!(
            digester.digest_of(&nested(5_000)),
            digester.digest_of(&nested(5_001))
        );
    }
}
