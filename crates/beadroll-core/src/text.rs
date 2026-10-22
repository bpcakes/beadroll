//! Text contracts used by bead's messages and keys (`docs/REVIEWING.md` §2): value
//! representations, escaping, whitespace trimming, base-36 digests and numeric parsing.

mod number;
mod printable;

pub use number::{decimal_value, is_digits, parse_float, parse_integer};

use std::fmt::Write as _;

use crate::json::{Value, float_repr};

/// Whether `c` can be shown unescaped by `repr`, according to the pinned printability table.
pub fn is_printable(c: char) -> bool {
    let code = u32::from(c);
    printable::NON_PRINTABLE
        .binary_search_by(|&(start, end)| {
            if end < code {
                std::cmp::Ordering::Less
            } else if start > code {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_err()
}

/// A quoted string: use single quotes unless it contains single but no double quotes. Escape the
/// delimiter, backslashes and nonprintable characters with short control escapes or `\xhh`,
/// `\uhhhh` and `\Uhhhhhhhh`.
pub fn repr_str(text: &str) -> String {
    repr_units(text.chars().map(Ok))
}

/// Decoded OS bytes: each valid UTF-8 character, or each invalid byte separately. The latter can
/// be represented by surrogateescape as U+DC00 plus the byte.
pub fn surrogateescaped(data: &[u8]) -> Vec<Result<char, u8>> {
    let mut units = Vec::with_capacity(data.len());
    let mut rest = data;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                units.extend(text.chars().map(Ok));
                break;
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                units.extend(
                    std::str::from_utf8(valid)
                        .unwrap_or_default()
                        .chars()
                        .map(Ok),
                );
                let bad = error.error_len().unwrap_or(after.len());
                units.extend(after.iter().take(bad).map(|b| Err(*b)));
                rest = after.get(bad..).unwrap_or_default();
            }
        }
    }
    units
}

/// A quoted representation of OS bytes. UTF-8 uses [`repr_str`] escaping; each invalid byte
/// becomes a `\udcXX` escape.
pub fn repr_bytes(data: &[u8]) -> String {
    repr_units(surrogateescaped(data).into_iter())
}

/// OS bytes for human output: valid UTF-8 unchanged, each invalid byte replaced by `?`.
pub fn printed_text(data: &[u8]) -> String {
    surrogateescaped(data)
        .into_iter()
        .map(|unit| unit.unwrap_or('?'))
        .collect()
}

/// OS bytes as an ASCII-escaped JSON string, including quotes. Escape controls and characters
/// outside printable ASCII, use surrogate pairs beyond the BMP and represent each invalid UTF-8
/// byte as U+FFFD (`\ufffd`), so that what bead prints or signs as JSON is JSON any reader takes
/// (until roll-lqjj.53 it was a lone surrogate escape, which only bead's own parser read).
pub fn json_ascii_bytes(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() + 2);
    out.push('"');
    for unit in surrogateescaped(data) {
        let c = unit.unwrap_or('\u{fffd}');
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut pair = [0u16; 2];
                for half in c.encode_utf16(&mut pair) {
                    let _ = write!(out, "\\u{half:04x}");
                }
            }
        }
    }
    out.push('"');
    out
}

/// `repr()` of a string made of characters and of bytes `surrogateescape` kept as lone surrogates.
fn repr_units(units: impl Iterator<Item = Result<char, u8>> + Clone) -> String {
    let has = |wanted: char| units.clone().any(|unit| unit == Ok(wanted));
    let quote = if has('\'') && !has('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for unit in units {
        let c = match unit {
            Ok(c) => c,
            Err(byte) => {
                let _ = write!(out, "\\u{:04x}", 0xDC00 | u32::from(byte));
                continue;
            }
        };
        let code = u32::from(c);
        match c {
            _ if c == quote || c == '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if code < 0x20 || code == 0x7F => {
                let _ = write!(out, "\\x{code:02x}");
            }
            _ if code < 0x7F || is_printable(c) => out.push(c),
            _ if code <= 0xFF => {
                let _ = write!(out, "\\x{code:02x}");
            }
            _ if code <= 0xFFFF => {
                let _ = write!(out, "\\u{code:04x}");
            }
            _ => {
                let _ = write!(out, "\\U{code:08x}");
            }
        }
    }
    out.push(quote);
    out
}

/// The established human-readable value representation: `None`, `True`, `1.5`, `'text'`, `[1,
/// 'a']`, `{'k': None}`.
pub fn repr(value: &Value) -> String {
    let mut out = String::new();
    write_repr(&mut out, value);
    out
}

fn write_repr(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("None"),
        Value::Bool(true) => out.push_str("True"),
        Value::Bool(false) => out.push_str("False"),
        Value::Int(int) => {
            let _ = write!(out, "{int}");
        }
        Value::Float(float) => {
            out.push_str(&float_repr(*float).unwrap_or_else(|_| "nan".to_owned()))
        }
        Value::Str(text) => out.push_str(&repr_str(text)),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_repr(out, item);
            }
            out.push(']');
        }
        Value::Object(members) => {
            out.push('{');
            for (index, (key, item)) in members.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&repr_str(key));
                out.push_str(": ");
                write_repr(out, item);
            }
            out.push('}');
        }
    }
}

/// A string unchanged, or any other value formatted with `repr`.
pub fn str_of(value: &Value) -> String {
    match value {
        Value::Str(text) => text.clone(),
        other => repr(other),
    }
}

/// Whether `c` is Unicode whitespace or a separator from U+001C through U+001F.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Text without surrounding whitespace, using [`is_space`].
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// Bytes without surrounding ASCII space, tab, newline, carriage return, vertical tab or form
/// feed.
pub fn strip_bytes(data: &[u8]) -> &[u8] {
    let space = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C);
    let start = data.iter().position(|b| !space(b)).unwrap_or(data.len());
    let end = data
        .iter()
        .rposition(|b| !space(b))
        .map_or(start, |at| at + 1);
    data.get(start..end).unwrap_or_default()
}

/// `text` as one word to a POSIX shell, as it is when every character is safe
/// there (ASCII letters, digits and `_@%+=:,./-`), else in single quotes, each one inside closed,
/// quoted and reopened.
pub fn shell_quote(text: &str) -> String {
    if text.is_empty() {
        return "''".to_owned();
    }
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if text.chars().all(safe) {
        return text.to_owned();
    }
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

/// A base-36 digest treating `bytes` as a big-endian integer and writing its least significant
/// digit first.
pub fn b36(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut number: Vec<u8> = bytes.iter().copied().skip_while(|b| *b == 0).collect();
    let mut out = String::new();
    while !number.is_empty() {
        let mut remainder = 0u32;
        for byte in &mut number {
            let current = (remainder << 8) | u32::from(*byte);
            *byte = (current / 36) as u8;
            remainder = current % 36;
        }
        out.push(char::from(
            DIGITS.get(remainder as usize).copied().unwrap_or(b'0'),
        ));
        let leading = number.iter().take_while(|b| **b == 0).count();
        number.drain(..leading);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    #[test]
    fn repr_quotes_strings_and_escapes_non_printable_characters() {
        assert_eq!(repr_str("a'b"), "\"a'b\"");
        assert_eq!(repr_str("a\"b"), "'a\"b'");
        assert_eq!(repr_str("a'\"b"), "'a\\'\"b'");
        let text = "\u{7f}\u{0}\u{200b}\u{a0}\u{378}é😀\u{e0001}\\\t\n\r";
        assert_eq!(
            repr_str(text),
            "'\\x7f\\x00\\u200b\\xa0\\u0378é😀\\U000e0001\\\\\\t\\n\\r'"
        );
        assert!(is_printable(' ') && is_printable('é') && !is_printable('\u{2028}'));
    }

    #[test]
    fn bytes_that_are_not_utf8_read_as_surrogateescape_makes_them() {
        // Invalid bytes remain separate surrogate escapes beside valid text and quotes.
        assert_eq!(repr_bytes(b"a\xe2\x82b'"), "\"a\\udce2\\udc82b'\"");
        assert_eq!(printed_text(b"a\xffb\xe2\x82"), "a?b??");
        // ASCII JSON escapes cover controls and Unicode; an invalid OS byte is U+FFFD.
        assert_eq!(
            json_ascii_bytes(b"\"\\\n\x7f\xc3\xa9\xf0\x9f\x98\x80\xff\x08\x0c\x01"),
            r#""\"\\\n\u007f\u00e9\ud83d\ude00\ufffd\b\f\u0001""#
        );
    }

    #[test]
    fn repr_and_str_of_json_values() {
        let value = parse(br#"[null, true, 1.0, {"k": "v"}, 18446744073709551615, 1e22]"#)
            .unwrap()
            .value;
        assert_eq!(
            repr(&value),
            "[None, True, 1.0, {'k': 'v'}, 18446744073709551615, 1e+22]"
        );
        assert_eq!(str_of(&Value::from("ex")), "ex");
        assert_eq!(str_of(&Value::Null), "None");
    }

    #[test]
    fn strips_use_unicode_whitespace_for_text_and_ascii_for_bytes() {
        assert_eq!(strip("\u{1c}\u{a0} x \u{3000}"), "x");
        assert_eq!(strip_bytes(b"\x0b\x0c x\t\n"), b"x");
        assert_eq!(strip_bytes(b" \xa0 "), b"\xa0");
        assert_eq!(strip_bytes(b"  "), b"");
    }

    #[test]
    fn shell_quote_preserves_each_argument_as_one_word() {
        // Shell quoting must preserve each argument as one word.
        assert_eq!(shell_quote("git@host:o/r.git"), "git@host:o/r.git");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(shell_quote("é"), "'é'");
    }

    #[test]
    fn b36_encodes_bytes_with_least_significant_digit_first() {
        // Zero and empty bytes both have an empty digest; 00ff10 gives sde1.
        assert_eq!(b36(&[0x00, 0xff, 0x10]), "sde1");
        assert_eq!(b36(&[]), "");
        assert_eq!(b36(&[0]), "");
        assert_eq!(b36(&[35]), "z");
        assert_eq!(b36(&[36]), "01");
    }
}
