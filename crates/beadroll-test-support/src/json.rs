//! A reader for what `bead --json` prints, until `beadroll-core` has the Python-compatible JSON
//! of `docs/RUST-PORT.md` §2.3a (Phase 1), which replaces it.

use std::ops::Index;

/// Nesting beyond this is refused rather than read: bead's own documents nest at most 100 levels.
const MAX_DEPTH: usize = 512;

/// A JSON document as `json.loads` reads bead's output: objects keep their keys in order, and a
/// repeated key keeps its last value in its first place, as a Python dict does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number exactly as written: bead's integers can be wider than any Rust integer.
    Number(String),
    /// A string. A lone surrogate escape, which a Rust string cannot hold, reads as U+FFFD.
    String(String),
    /// An array.
    Array(Vec<Json>),
    /// An object's members, in order.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Reads one JSON document, with nothing but whitespace around it.
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut reader = Reader {
            text,
            at: 0,
            depth: 0,
        };
        reader.skip_space();
        let value = reader.value()?;
        reader.skip_space();
        if reader.at == text.len() {
            Ok(value)
        } else {
            Err(reader.error("data after the document"))
        }
    }

    /// The value of `key`, if this is an object that has it.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The text, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(text) => Some(text),
            _ => None,
        }
    }

    /// The items, if this is an array.
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The members, if this is an object.
    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(members) => Some(members),
            _ => None,
        }
    }

    /// The value, if this is `true` or `false`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The value, if this is an integer that fits an `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Number(text) => text.parse().ok(),
            _ => None,
        }
    }
}

/// `document["key"]`: the value of `key`; panics if this is not an object that has it.
impl Index<&str> for Json {
    type Output = Json;

    fn index(&self, key: &str) -> &Json {
        self.get(key)
            .unwrap_or_else(|| panic!("no key {key:?} in {self:?}"))
    }
}

/// `document[i]`: item `i`; panics if this is not an array that long.
impl Index<usize> for Json {
    type Output = Json;

    fn index(&self, index: usize) -> &Json {
        self.as_array()
            .and_then(|items| items.get(index))
            .unwrap_or_else(|| panic!("no item {index} in {self:?}"))
    }
}

struct Reader<'t> {
    text: &'t str,
    at: usize,
    depth: usize,
}

impl Reader<'_> {
    fn error(&self, what: &str) -> String {
        format!("{what} at byte {}", self.at)
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    fn skip_space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.at += 1;
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Json::String),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.error("expected a value")),
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.text[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("expected a value"))
        }
    }

    /// Enters an object or array: past its opening bracket, one level deeper.
    fn open(&mut self) -> Result<(), String> {
        if self.depth == MAX_DEPTH {
            return Err(self.error("too deeply nested"));
        }
        self.depth += 1;
        self.at += 1;
        self.skip_space();
        Ok(())
    }

    /// After an item: past a comma (true, another item follows) or the closing `end` (false).
    fn next_item(&mut self, end: u8) -> Result<bool, String> {
        self.skip_space();
        match self.peek() {
            Some(b',') => {
                self.at += 1;
                self.skip_space();
                Ok(true)
            }
            Some(byte) if byte == end => {
                self.at += 1;
                self.depth -= 1;
                Ok(false)
            }
            _ => Err(self.error("expected ',' or the end of the container")),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.open()?;
        let mut members: Vec<(String, Json)> = Vec::new();
        if self.peek() == Some(b'}') {
            self.at += 1;
            self.depth -= 1;
            return Ok(Json::Object(members));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a key"));
            }
            let key = self.string()?;
            self.skip_space();
            if self.peek() != Some(b':') {
                return Err(self.error("expected ':'"));
            }
            self.at += 1;
            self.skip_space();
            let value = self.value()?;
            match members.iter_mut().find(|(k, _)| *k == key) {
                Some(member) => member.1 = value,
                None => members.push((key, value)),
            }
            if !self.next_item(b'}')? {
                return Ok(Json::Object(members));
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.open()?;
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.at += 1;
            self.depth -= 1;
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value()?);
            if !self.next_item(b']')? {
                return Ok(Json::Array(items));
            }
        }
    }

    fn digits(&mut self) -> bool {
        let start = self.at;
        while let Some(b'0'..=b'9') = self.peek() {
            self.at += 1;
        }
        self.at > start
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.error("expected a digit")),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if !self.digits() {
                return Err(self.error("expected a digit"));
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.at += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.at += 1;
            }
            if !self.digits() {
                return Err(self.error("expected a digit"));
            }
        }
        Ok(Json::Number(self.text[start..self.at].to_owned()))
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut text = String::new();
        loop {
            // A run of plain characters ends only at an ASCII byte, so it is whole UTF-8.
            let start = self.at;
            while let Some(byte) = self.peek() {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.at += 1;
            }
            text.push_str(&self.text[start..self.at]);
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(text);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escaped = self.escape()?;
                    text.push(escaped);
                }
                Some(_) => return Err(self.error("a control character in a string")),
                None => return Err(self.error("an unterminated string")),
            }
        }
    }

    fn escape(&mut self) -> Result<char, String> {
        let byte = self
            .peek()
            .ok_or_else(|| self.error("an unterminated escape"))?;
        self.at += 1;
        Ok(match byte {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let unit = self.hex4()?;
                let pair =
                    (0xD800..0xDC00).contains(&unit) && self.text[self.at..].starts_with("\\u");
                let low = if pair {
                    let before = self.at;
                    self.at += 2;
                    let low = self.hex4()?;
                    if (0xDC00..0xE000).contains(&low) {
                        Some(low)
                    } else {
                        self.at = before; // not its pair: read it as an escape of its own
                        None
                    }
                } else {
                    None
                };
                let code = match low {
                    Some(low) => 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00),
                    None => unit,
                };
                char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)
            }
            _ => return Err(self.error("an unknown escape")),
        })
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .text
            .get(self.at..self.at + 4)
            .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| self.error("a \\u escape without four hex digits"))?;
        let unit =
            u32::from_str_radix(digits, 16).map_err(|error| self.error(&error.to_string()))?;
        self.at += 4;
        Ok(unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_beads_output_in_order() {
        let doc = Json::parse(
            r#"{"id": "ex-1", "labels": ["a", "b"], "claim": null, "n": -12, "ok": true}"#,
        )
        .unwrap();
        let keys: Vec<_> = doc
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, ["id", "labels", "claim", "n", "ok"]);
        assert_eq!(doc["id"].as_str(), Some("ex-1"));
        assert_eq!(doc["labels"][1].as_str(), Some("b"));
        assert_eq!(doc["claim"], Json::Null);
        assert_eq!(doc["n"].as_i64(), Some(-12));
        assert_eq!(doc["ok"].as_bool(), Some(true));
    }

    #[test]
    fn a_repeated_key_keeps_its_last_value_in_its_first_place() {
        let doc = Json::parse(r#"{"a": 1, "b": 2, "a": 3}"#).unwrap();
        assert_eq!(
            doc,
            Json::Object(vec![
                ("a".into(), Json::Number("3".into())),
                ("b".into(), Json::Number("2".into())),
            ])
        );
    }

    #[test]
    fn numbers_keep_their_text() {
        let wide = "123456789012345678901234567890";
        assert_eq!(Json::parse(wide).unwrap(), Json::Number(wide.into()));
        assert_eq!(Json::parse("1e-05").unwrap(), Json::Number("1e-05".into()));
        assert_eq!(Json::parse(wide).unwrap().as_i64(), None);
    }

    #[test]
    fn escapes_decode_and_surrogate_pairs_join() {
        let doc =
            Json::parse(r#""\u001b[0m \"\\\/\b\f\n\r\t \u00e9 \ud83d\ude00 \ud800 \udc00 é""#)
                .unwrap();
        assert_eq!(
            doc.as_str(),
            Some("\u{1b}[0m \"\\/\u{8}\u{c}\n\r\t é 😀 \u{fffd} \u{fffd} é")
        );
        let unpaired = Json::parse(r#""\ud800\u0041""#).unwrap();
        assert_eq!(unpaired.as_str(), Some("\u{fffd}A"));
    }

    #[test]
    fn malformed_documents_are_refused() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "{\"a\": 1,}",
            "01",
            "1.",
            "-",
            "1e",
            "tru",
            "\"\u{1}\"",
            "\"\\x\"",
            "\"\\u12\"",
            "\"abc",
            "[1] 2",
            "nan",
            "{1: 2}",
        ] {
            assert!(Json::parse(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = "[".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 1);
        assert!(
            Json::parse(&deep)
                .unwrap_err()
                .contains("too deeply nested")
        );
        let fine = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
        assert!(Json::parse(&fine).is_ok());
    }
}
