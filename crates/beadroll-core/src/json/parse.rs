//! Reading JSON as Python bead's `loads_strict` reads bytes: decoded as UTF-8 with lone surrogates
//! kept, refused when nested deeper than [`MAX_DEPTH`], then parsed as CPython's `json` scanner
//! parses (its error messages and positions included), with `NaN`, `Infinity` and non-finite numbers
//! refused and integers limited to [`MAX_DIGITS`] digits.

use indexmap::IndexMap;

use super::{Int, MAX_DIGITS, Object, Value};

/// The deepest arrays and objects nest in a text bead reads: every supported Python reads this far
/// (`docs/RUST-PORT.md` §5 row 7, decided 2026-10-09).
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

/// Why a text is not JSON bead reads. `Display` is Python's message.
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
    /// Not JSON: CPython's message, then where, in characters.
    #[error("{message}: line {line} column {column} (char {position})")]
    Syntax {
        /// What CPython's scanner says.
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

/// How deeply `bytes` nest arrays and objects, read as Python's `nesting` reads them: a string is one
/// token whatever brackets it holds, and nothing is checked.
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

/// UTF-8 as Python decodes it with `surrogatepass`: an encoded surrogate (`ED A0 80` to `ED BF BF`)
/// passes, anything else invalid does not.
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

/// An open container, while its contents are read. Every value carries whether it holds a lone
/// surrogate: Python reports one only if it survives in what is read, not in a value a repeated key
/// replaced.
enum Frame {
    Array { items: Vec<Value>, surrogate: bool },
    Object { members: Members, key: Key },
}

/// A key read from the text, kept as its bytes when it holds a lone surrogate: Python merges repeated
/// keys first as written, and only then as read with U+FFFD.
#[derive(Default)]
struct Key {
    text: String,
    raw: Option<Vec<u8>>,
}

/// An object's members as they are read: by key as written, each with its key's text, its value, and
/// whether key or value holds a lone surrogate.
#[derive(Default)]
struct Members(IndexMap<Vec<u8>, (String, Value, bool)>);

impl Members {
    fn insert(&mut self, key: Key, value: Value, value_surrogate: bool) {
        let surrogate = value_surrogate || key.raw.is_some();
        let raw = key.raw.unwrap_or_else(|| key.text.as_bytes().to_vec());
        match self.0.get_mut(&raw) {
            Some(member) => {
                member.1 = value;
                member.2 = surrogate;
            }
            None => {
                self.0.insert(raw, (key.text, value, surrogate));
            }
        }
    }

    /// The object as read: members merged again once each lone surrogate reads as U+FFFD.
    fn finish(self) -> (Object, bool) {
        let mut out = Object::with_capacity(self.0.len());
        let mut surrogate = false;
        for (_, (text, value, flag)) in self.0 {
            surrogate |= flag;
            out.insert(text, value);
        }
        (out, surrogate)
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

    /// A syntax error at byte `at`, with CPython's line, column and character position.
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
                        (Value::Object(Object::new()), false, inside + 1)
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
                                    (value, surrogate, end) =
                                        (Value::Object(object), any, after + 1);
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
                text: text(&raw),
                raw: Some(raw),
            }
        } else {
            Key {
                text: text(&raw),
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
                Ok((Value::Str(text(&raw)), surrogate, end))
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
            // CPython 3.14 refuses an escape that runs past the end; one that ends there is read,
            // and the string is then unterminated.
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

    /// The bound CPython compares an escape's end with: the text's length. In bytes it is the same
    /// bound wherever an escape is ASCII, which is the only place it is used.
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

/// The text of a string's bytes, each lone surrogate read as U+FFFD.
fn text(raw: &[u8]) -> String {
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
