//! Strict JSON reading with surrogate-aware UTF-8 decoding and nesting limited to [`MAX_DEPTH`].
//! Reject NaN, Infinity and non-finite numbers; limit integers to [`MAX_DIGITS`] digits. Error
//! messages and character positions follow the established JSON contract.

use indexmap::IndexMap;

use super::{Int, MAX_DIGITS, Object, Value};

/// Maximum array and object nesting accepted by readers, below every Python's own limit (U03 in
/// `docs/FINDINGS.md`).
pub const MAX_DEPTH: usize = 512;

/// A text read as JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    /// The value, every lone surrogate read as U+FFFD.
    pub value: Value,
    /// Whether the text held a lone surrogate (escaped, or as the raw bytes `ED A0 80` and the like),
    /// which bead reports: such text can be neither printed nor exported.
    pub lone_surrogates: bool,
}

/// Why a text cannot be read as JSON.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// Not UTF-8, even allowing lone surrogates.
    #[error("text is not valid UTF-8")]
    Utf8,
    /// Arrays and objects nest deeper than [`MAX_DEPTH`].
    #[error("nested deeper than {MAX_DEPTH} levels")]
    TooDeep,
    /// `NaN`, `Infinity`, `-Infinity`, or a number too large for a float.
    #[error("{0} is not a JSON number")]
    NotANumber(String),
    /// An integer with more digits than [`MAX_DIGITS`], its count.
    #[error(
        "Exceeds the limit ({MAX_DIGITS} digits) for integer string conversion: value has {0} digits; \
         use sys.set_int_max_str_digits() to increase the limit"
    )]
    IntDigits(usize),
    /// A JSON syntax diagnostic and its position in characters.
    #[error("{message}: line {line} column {column} (char {position})")]
    Syntax {
        /// The syntax diagnostic.
        message: &'static str,
        /// The line, from 1.
        line: usize,
        /// The column, from 1.
        column: usize,
        /// The position, in characters from 0.
        position: usize,
    },
}

/// Reads `bytes` as `loads_strict` does.
pub fn parse(bytes: &[u8]) -> Result<Parsed, ParseError> {
    check_utf8(bytes)?;
    if exceeds_depth(bytes) {
        return Err(ParseError::TooDeep);
    }
    let mut parser = Parser { bytes };
    if bytes.starts_with("\u{feff}".as_bytes()) {
        return Err(parser.syntax("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0));
    }
    let start = parser.skip_space(0);
    let (value, lone_surrogates, end) = parser.document(start)?;
    let end = parser.skip_space(end);
    if end != bytes.len() {
        return Err(parser.syntax("Extra data", end));
    }
    Ok(Parsed {
        value,
        lone_surrogates,
    })
}

/// Counts array and object nesting in bytes without validating syntax. A quoted string is one
/// token regardless of brackets inside it.
pub fn nesting(bytes: &[u8]) -> usize {
    let (mut depth, mut deepest, mut at) = (0isize, 0isize, 0);
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'"' => at = string_token_end(bytes, at + 1),
            b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
                at += 1;
            }
            b']' | b'}' => {
                depth -= 1;
                at += 1;
            }
            _ => at += 1,
        }
    }
    usize::try_from(deepest).unwrap_or(0)
}

fn exceeds_depth(bytes: &[u8]) -> bool {
    let brackets = bytes.iter().filter(|b| matches!(b, b'[' | b'{')).count();
    brackets > MAX_DEPTH && nesting(bytes) > MAX_DEPTH
}

/// Past a string token begun before `at`, as `"(?:[^"\\]|\\.)*"?` matches it: through the closing
/// quote, or to the end. A backslash takes the next byte with it; the rest of a multi-byte character
/// is never a quote or a backslash, so bytes and characters agree.
fn string_token_end(bytes: &[u8], mut at: usize) -> usize {
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'"' => return at + 1,
            b'\\' => at += 2,
            _ => at += 1,
        }
    }
    bytes.len()
}

/// UTF-8 decoding that permits encoded surrogates (`ED A0 80` through `ED BF BF`) but rejects
/// other invalid sequences.
fn check_utf8(bytes: &[u8]) -> Result<(), ParseError> {
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(_) => return Ok(()),
            Err(error) => {
                let bad = error.valid_up_to();
                match rest.get(bad..bad + 3) {
                    Some([0xED, 0xA0..=0xBF, 0x80..=0xBF]) => {
                        rest = rest.get(bad + 3..).unwrap_or_default()
                    }
                    _ => return Err(ParseError::Utf8),
                }
            }
        }
    }
}

/// An open container during parsing. Each value tracks lone surrogates so only those surviving
/// duplicate-key replacement are reported.
enum Frame {
    Array { items: Vec<Value>, surrogate: bool },
    Object { members: Members, key: Key },
}

/// A parsed key, retaining raw bytes if it contains a lone surrogate. Merge repeated keys first
/// by original spelling, then by decoded spelling with U+FFFD.
#[derive(Default)]
struct Key {
    text: String,
    raw: Option<Vec<u8>>,
}

/// An object's members as they are read. A key without a lone surrogate reads as it was written,
/// so while every key is such the object is built once, by key as read, as it will be returned;
/// the first key holding a lone surrogate turns the members into [`Members::Raw`], by key as
/// written, merged again by key as read once the object closes, since two keys written apart may
/// read alike (roll-lqjj.49: every object was built by key as written and then again by key as
/// read, 31 ms of the 52 ms a `meta.json` of 92,000 files took).
enum Members {
    /// Every key read so far reads as written: the object itself, and for each member whether
    /// its value holds a lone surrogate, so that one a repeated key replaced is not reported.
    Plain {
        members: Object,
        surrogates: Vec<bool>,
    },
    /// A key held a lone surrogate: by key as written, each with its key's text, its value and
    /// whether key or value holds a lone surrogate.
    Raw(IndexMap<Vec<u8>, (String, Value, bool)>),
}

impl Default for Members {
    fn default() -> Self {
        Members::Plain {
            members: Object::new(),
            surrogates: Vec::new(),
        }
    }
}

impl Members {
    fn insert(&mut self, key: Key, value: Value, value_surrogate: bool) {
        let surrogate = value_surrogate || key.raw.is_some();
        match self {
            Members::Plain {
                members,
                surrogates,
            } if key.raw.is_none() => match members.get_full_mut(key.text.as_str()) {
                Some((at, _, member)) => {
                    *member = value;
                    if let Some(flag) = surrogates.get_mut(at) {
                        *flag = surrogate;
                    }
                }
                None => {
                    members.insert(key.text, value);
                    surrogates.push(surrogate);
                }
            },
            Members::Plain { .. } => {
                // The first key with a lone surrogate: what was read so far is keyed by its text's
                // bytes, which are its bytes as written, and this key and the rest by theirs.
                let Members::Plain {
                    members,
                    surrogates,
                } = std::mem::take(self)
                else {
                    return;
                };
                let mut raw = IndexMap::with_capacity(members.len() + 1);
                for ((text, value), flag) in members.into_iter().zip(surrogates) {
                    raw.insert(text.clone().into_bytes(), (text, value, flag));
                }
                *self = Members::Raw(raw);
                self.insert(key, value, value_surrogate);
            }
            Members::Raw(raw) => {
                let bytes = key.raw.unwrap_or_else(|| key.text.as_bytes().to_vec());
                match raw.get_mut(&bytes) {
                    Some(member) => {
                        member.1 = value;
                        member.2 = surrogate;
                    }
                    None => {
                        raw.insert(bytes, (key.text, value, surrogate));
                    }
                }
            }
        }
    }

    /// The object as read: members merged again once each lone surrogate reads as U+FFFD.
    fn finish(self) -> (Object, bool) {
        match self {
            Members::Plain {
                members,
                surrogates,
            } => (members, surrogates.into_iter().any(|flag| flag)),
            Members::Raw(raw) => {
                let mut out = Object::with_capacity(raw.len());
                let mut surrogate = false;
                for (_, (text, value, flag)) in raw {
                    surrogate |= flag;
                    out.insert(text, value);
                }
                (out, surrogate)
            }
        }
    }
}

struct Parser<'t> {
    bytes: &'t [u8],
}

impl Parser<'_> {
    fn at(&self, at: usize) -> Option<u8> {
        self.bytes.get(at).copied()
    }

    fn skip_space(&self, mut at: usize) -> usize {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.at(at) {
            at += 1;
        }
        at
    }

    /// A syntax error at byte `at`, reported with line, column and character position.
    fn syntax(&self, message: &'static str, at: usize) -> ParseError {
        let before = self.bytes.get(..at).unwrap_or(self.bytes);
        let position = before.iter().filter(|b| (**b & 0xC0) != 0x80).count();
        let line = before.iter().filter(|b| **b == b'\n').count() + 1;
        let line_start = before
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |n| n + 1);
        let column = before
            .get(line_start..)
            .unwrap_or_default()
            .iter()
            .filter(|b| (**b & 0xC0) != 0x80)
            .count()
            + 1;
        ParseError::Syntax {
            message,
            line,
            column,
            position,
        }
    }

    fn starts_with(&self, at: usize, word: &[u8]) -> bool {
        self.bytes
            .get(at..)
            .is_some_and(|rest| rest.starts_with(word))
    }

    /// The value starting at `start`, whether it holds a lone surrogate, and where it ends; containers
    /// are read without recursion.
    fn document(&mut self, start: usize) -> Result<(Value, bool, usize), ParseError> {
        let mut stack: Vec<Frame> = Vec::new();
        let mut at = start;
        loop {
            let (mut value, mut surrogate, mut end) = match self.at(at) {
                Some(b'{') => {
                    let inside = self.skip_space(at + 1);
                    if self.at(inside) == Some(b'}') {
                        (Value::from(Object::new()), false, inside + 1)
                    } else {
                        let (key, next) = self.key(inside)?;
                        stack.push(Frame::Object {
                            members: Members::default(),
                            key,
                        });
                        at = next;
                        continue;
                    }
                }
                Some(b'[') => {
                    let inside = self.skip_space(at + 1);
                    if self.at(inside) == Some(b']') {
                        (Value::Array(Vec::new()), false, inside + 1)
                    } else {
                        stack.push(Frame::Array {
                            items: Vec::new(),
                            surrogate: false,
                        });
                        at = inside;
                        continue;
                    }
                }
                _ => self.scalar(at)?,
            };
            // Close every container this value completes; stop where the next value starts.
            loop {
                let after = self.skip_space(end);
                match stack.last_mut() {
                    None => return Ok((value, surrogate, end)),
                    Some(Frame::Array {
                        items,
                        surrogate: any,
                    }) => {
                        items.push(value);
                        *any |= surrogate;
                        match self.at(after) {
                            Some(b']') => match stack.pop() {
                                Some(Frame::Array {
                                    items,
                                    surrogate: any,
                                }) => {
                                    (value, surrogate, end) = (Value::Array(items), any, after + 1);
                                }
                                _ => return Err(self.syntax("Expecting value", after)),
                            },
                            Some(b',') => {
                                let next = self.skip_space(after + 1);
                                if self.at(next) == Some(b']') {
                                    return Err(self.syntax(
                                        "Illegal trailing comma before end of array",
                                        after,
                                    ));
                                }
                                at = next;
                                break;
                            }
                            _ => return Err(self.syntax("Expecting ',' delimiter", after)),
                        }
                    }
                    Some(Frame::Object { members, key }) => {
                        members.insert(std::mem::take(key), value, surrogate);
                        match self.at(after) {
                            Some(b'}') => match stack.pop() {
                                Some(Frame::Object { members, .. }) => {
                                    let (object, any) = members.finish();
                                    (value, surrogate, end) = (Value::from(object), any, after + 1);
                                }
                                _ => return Err(self.syntax("Expecting value", after)),
                            },
                            Some(b',') => {
                                let next = self.skip_space(after + 1);
                                if self.at(next) == Some(b'}') {
                                    return Err(self.syntax(
                                        "Illegal trailing comma before end of object",
                                        after,
                                    ));
                                }
                                let (next_key, value_at) = self.key(next)?;
                                if let Some(Frame::Object { key, .. }) = stack.last_mut() {
                                    *key = next_key;
                                }
                                at = value_at;
                                break;
                            }
                            _ => return Err(self.syntax("Expecting ',' delimiter", after)),
                        }
                    }
                }
            }
        }
    }

    /// A member's key at `at`, then its colon: the key and where its value starts.
    fn key(&mut self, at: usize) -> Result<(Key, usize), ParseError> {
        if self.at(at) != Some(b'"') {
            return Err(self.syntax("Expecting property name enclosed in double quotes", at));
        }
        let (raw, surrogate, end) = self.string(at + 1)?;
        let after = self.skip_space(end);
        if self.at(after) != Some(b':') {
            return Err(self.syntax("Expecting ':' delimiter", after));
        }
        let key = if surrogate {
            Key {
                text: replaced(&raw),
                raw: Some(raw),
            }
        } else {
            Key {
                text: text(raw),
                raw: None,
            }
        };
        Ok((key, self.skip_space(after + 1)))
    }

    /// A string, literal, constant or number at `at`, whether it holds a lone surrogate, and where it
    /// ends.
    fn scalar(&mut self, at: usize) -> Result<(Value, bool, usize), ParseError> {
        match self.at(at) {
            Some(b'"') => {
                let (raw, surrogate, end) = self.string(at + 1)?;
                let text = if surrogate { replaced(&raw) } else { text(raw) };
                Ok((Value::Str(text), surrogate, end))
            }
            Some(b'n') if self.starts_with(at, b"null") => Ok((Value::Null, false, at + 4)),
            Some(b't') if self.starts_with(at, b"true") => Ok((Value::Bool(true), false, at + 4)),
            Some(b'f') if self.starts_with(at, b"false") => Ok((Value::Bool(false), false, at + 5)),
            Some(b'N') if self.starts_with(at, b"NaN") => Err(ParseError::NotANumber("NaN".into())),
            Some(b'I') if self.starts_with(at, b"Infinity") => {
                Err(ParseError::NotANumber("Infinity".into()))
            }
            Some(b'-') if self.starts_with(at, b"-Infinity") => {
                Err(ParseError::NotANumber("-Infinity".into()))
            }
            _ => self.number(at).map(|(value, end)| (value, false, end)),
        }
    }

    fn digits(&self, mut at: usize) -> usize {
        while let Some(b'0'..=b'9') = self.at(at) {
            at += 1;
        }
        at
    }

    /// A number as `_match_number_unicode` matches it: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`.
    fn number(&self, start: usize) -> Result<(Value, usize), ParseError> {
        let mut at = start;
        if self.at(at) == Some(b'-') {
            at += 1;
        }
        match self.at(at) {
            Some(b'1'..=b'9') => at = self.digits(at + 1),
            Some(b'0') => at += 1,
            _ => return Err(self.syntax("Expecting value", start)),
        }
        let mut float = false;
        if self.at(at) == Some(b'.') && matches!(self.at(at + 1), Some(b'0'..=b'9')) {
            float = true;
            at = self.digits(at + 2);
        }
        if let Some(b'e' | b'E') = self.at(at) {
            let mut exponent = at + 1;
            if let Some(b'-' | b'+') = self.at(exponent) {
                exponent += 1;
            }
            let end = self.digits(exponent);
            if end > exponent {
                float = true;
                at = end;
            }
        }
        let text =
            std::str::from_utf8(self.bytes.get(start..at).unwrap_or_default()).unwrap_or_default();
        if float {
            match text.parse::<f64>() {
                Ok(value) if value.is_finite() => Ok((Value::Float(value), at)),
                _ => Err(ParseError::NotANumber(text.to_owned())),
            }
        } else {
            let count = text.trim_start_matches('-').len();
            if count > MAX_DIGITS {
                return Err(ParseError::IntDigits(count));
            }
            Int::parse(text)
                .map(|int| (Value::Int(int), at))
                .ok_or(ParseError::IntDigits(count))
        }
    }

    /// The contents of a string opened before `start`, as `scanstring_unicode` reads them: its bytes
    /// (an escaped surrogate encoded as UTF-8 would encode it), whether it holds a lone surrogate, and
    /// where it ends.
    fn string(&mut self, start: usize) -> Result<(Vec<u8>, bool, usize), ParseError> {
        let opened = start - 1;
        let mut out = Vec::new();
        let mut surrogate = false;
        let mut at = start;
        loop {
            let chunk = at;
            loop {
                match self.at(at) {
                    None => return Err(self.syntax("Unterminated string starting at", opened)),
                    Some(b'"' | b'\\') => break,
                    Some(byte) if byte <= 0x1f => {
                        return Err(self.syntax("Invalid control character at", at));
                    }
                    Some(0xED) if matches!(self.at(at + 1), Some(0xA0..=0xBF)) => {
                        surrogate = true;
                        at += 1;
                    }
                    Some(_) => at += 1,
                }
            }
            out.extend_from_slice(self.bytes.get(chunk..at).unwrap_or_default());
            if self.at(at) == Some(b'"') {
                return Ok((out, surrogate, at + 1));
            }
            // A backslash at `at`.
            let escape = at + 1;
            let Some(kind) = self.at(escape) else {
                return Err(self.syntax("Unterminated string starting at", opened));
            };
            if kind != b'u' {
                let decoded = match kind {
                    b'"' => '"',
                    b'\\' => '\\',
                    b'/' => '/',
                    b'b' => '\u{8}',
                    b'f' => '\u{c}',
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    _ => return Err(self.syntax("Invalid \\escape", at)),
                };
                push_char(&mut out, u32::from(decoded));
                at = escape + 1;
                continue;
            }
            let hex = escape + 1;
            let mut end = hex + 4;
            // An escape extending past the end is invalid. One ending there is read, then the
            // string is reported as unterminated.
            if end > self.char_end_limit() {
                return Err(self.syntax("Invalid \\uXXXX escape", escape));
            }
            let mut unit = self
                .hex4(hex)
                .ok_or_else(|| self.syntax("Invalid \\uXXXX escape", escape))?;
            if (0xD800..0xDC00).contains(&unit)
                && end + 6 < self.char_end_limit()
                && self.at(end) == Some(b'\\')
                && self.at(end + 1) == Some(b'u')
            {
                let low = self
                    .hex4(end + 2)
                    .ok_or_else(|| self.syntax("Invalid \\uXXXX escape", end + 1))?;
                if (0xDC00..0xE000).contains(&low) {
                    unit = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                    end += 6;
                }
            }
            if (0xD800..0xE000).contains(&unit) {
                surrogate = true;
            }
            push_char(&mut out, unit);
            at = end;
        }
    }

    /// The text length used to bound an escape. Byte and character bounds agree for ASCII
    /// escapes, the only case using this helper.
    fn char_end_limit(&self) -> usize {
        self.bytes.len()
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        let digits = self.bytes.get(at..at + 4)?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
    }
}

/// The text of a string's bytes that hold no lone surrogate: the bytes themselves, which `string`
/// read as UTF-8, taken as they are rather than copied (roll-lqjj.49).
fn text(raw: Vec<u8>) -> String {
    String::from_utf8(raw)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

/// The text of a string's bytes, each lone surrogate read as U+FFFD.
fn replaced(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut at = 0;
    while let Some(&byte) = raw.get(at) {
        if byte == 0xED && matches!(raw.get(at + 1), Some(0xA0..=0xBF)) {
            out.extend_from_slice("\u{fffd}".as_bytes());
            at += 3;
        } else {
            out.push(byte);
            at += 1;
        }
    }
    String::from_utf8(out)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

/// Appends code point `unit` as UTF-8 would encode it, a surrogate included.
fn push_char(out: &mut Vec<u8>, unit: u32) {
    match char::from_u32(unit) {
        Some(c) => {
            let mut buffer = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
        }
        None => {
            // A surrogate, U+D800 to U+DFFF: three bytes, ED then two continuation bytes.
            out.push(0xED);
            out.push(0x80 | ((unit >> 6) & 0x3F) as u8);
            out.push(0x80 | (unit & 0x3F) as u8);
        }
    }
}

#[cfg(test)]
mod tests;
