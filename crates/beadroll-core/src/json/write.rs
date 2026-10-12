//! Writing JSON as Python's `json.dumps` writes it, in each dialect bead uses, floats as `repr()`
//! prints them.

use std::fmt::Write as _;

use super::{MAX_DIGITS, Value};

/// How members and items are separated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// `separators=(",", ":")`.
    Compact,
    /// Python's default separators, `", "` and `": "`.
    Spaced,
    /// `indent=1`: one item per line, one space of indent per level, `","` and `": "`.
    Indented,
}

/// A `json.dumps` dialect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// The separators and line layout.
    pub style: Style,
    /// `ensure_ascii`: everything outside printable ASCII escaped.
    pub ascii: bool,
    /// `sort_keys`: object members in key order instead of insertion order.
    pub sorted: bool,
}

impl Format {
    /// Journal lines, attempt files and Beads export: compact, raw UTF-8.
    pub const COMPACT: Format = Format {
        style: Style::Compact,
        ascii: false,
        sorted: false,
    };
    /// `--json` results: spaced, raw UTF-8.
    pub const SPACED: Format = Format {
        style: Style::Spaced,
        ascii: false,
        sorted: false,
    };
    /// Errors, `--version` and `replica.json`: spaced, ASCII-escaped.
    pub const ASCII: Format = Format {
        style: Style::Spaced,
        ascii: true,
        sorted: false,
    };
    /// `meta.json`, `aliases.json`, `moved.json` and `.beadroll.json`: `indent=1`, ASCII-escaped.
    pub const INDENTED: Format = Format {
        style: Style::Indented,
        ascii: true,
        sorted: false,
    };
    /// The quarantine comparison: sorted keys, compact, raw UTF-8.
    pub const CANONICAL: Format = Format {
        style: Style::Compact,
        ascii: false,
        sorted: true,
    };
    /// The published manifest and command receipts: sorted keys, spaced, ASCII-escaped.
    pub const SORTED_ASCII: Format = Format {
        style: Style::Spaced,
        ascii: true,
        sorted: true,
    };
}

/// Why a value cannot be written. `Display` is Python's message.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    /// A float that is not finite, built in memory: no reader would take it back.
    #[error("Out of range float values are not JSON compliant: {0}")]
    NotFinite(&'static str),
    /// An integer with more than [`MAX_DIGITS`] digits, built in memory: Python would not read it.
    #[error(
        "Exceeds the limit ({MAX_DIGITS} digits) for integer string conversion; \
         use sys.set_int_max_str_digits() to increase the limit"
    )]
    IntDigits,
    /// Nesting beyond what any reader takes back.
    #[error("nested deeper than any reader takes back")]
    TooDeep,
}

/// The deepest a value may nest to be written: twice what is read, so that a value read and wrapped
/// a few levels deeper is still written, and nothing built in memory recurses without bound.
const WRITE_DEPTH: usize = 2 * super::MAX_DEPTH;

/// `value` as `json.dumps` writes it in `format`.
pub fn write(value: &Value, format: Format) -> Result<String, WriteError> {
    let mut out = String::new();
    write_value(&mut out, value, format, 0)?;
    Ok(out)
}

fn write_value(
    out: &mut String,
    value: &Value,
    format: Format,
    level: usize,
) -> Result<(), WriteError> {
    if level > WRITE_DEPTH {
        return Err(WriteError::TooDeep);
    }
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(int) => {
            if int.digit_count() > MAX_DIGITS {
                return Err(WriteError::IntDigits);
            }
            let _ = write!(out, "{int}");
        }
        Value::Float(float) => out.push_str(&float_repr(*float)?),
        Value::Str(text) => write_str(out, text, format.ascii),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return Ok(());
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                separate(out, format.style, index, level + 1);
                write_value(out, item, format, level + 1)?;
            }
            close(out, format.style, level, ']');
        }
        Value::Object(members) => {
            if members.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            out.push('{');
            let mut entries: Vec<(&String, &Value)> = members.iter().collect();
            if format.sorted {
                entries.sort_by(|a, b| a.0.cmp(b.0));
            }
            for (index, (key, item)) in entries.into_iter().enumerate() {
                separate(out, format.style, index, level + 1);
                write_str(out, key, format.ascii);
                out.push_str(if format.style == Style::Compact {
                    ":"
                } else {
                    ": "
                });
                write_value(out, item, format, level + 1)?;
            }
            close(out, format.style, level, '}');
        }
    }
    Ok(())
}

/// Before item `index` of a container at `level`: the item separator, and the line and indent.
fn separate(out: &mut String, style: Style, index: usize, level: usize) {
    match style {
        Style::Compact if index > 0 => out.push(','),
        Style::Spaced if index > 0 => out.push_str(", "),
        Style::Indented => {
            if index > 0 {
                out.push(',');
            }
            out.push('\n');
            out.extend(std::iter::repeat_n(' ', level));
        }
        _ => {}
    }
}

fn close(out: &mut String, style: Style, level: usize, bracket: char) {
    if style == Style::Indented {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', level));
    }
    out.push(bracket);
}

/// A string as `json.dumps` quotes it: quote, backslash and control characters escaped, and with
/// `ascii` everything outside `' '..='~'` too, astral characters as surrogate pairs.
pub(crate) fn write_str(out: &mut String, text: &str, ascii: bool) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c < ' ' || (ascii && c > '~') => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A float as Python's `repr()` prints it: the shortest digits that read back as the same float, in
/// positional notation from 1e-4 up to 1e16 (with `.0` if there is no fraction), otherwise as
/// `d.ddde±XX`.
pub fn float_repr(value: f64) -> Result<String, WriteError> {
    if value.is_nan() {
        return Err(WriteError::NotFinite("nan"));
    }
    if value.is_infinite() {
        return Err(WriteError::NotFinite(if value > 0.0 {
            "inf"
        } else {
            "-inf"
        }));
    }
    if value == 0.0 {
        return Ok(if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .to_owned());
    }
    let (digits, exponent) = shortest_digits(value.abs());
    let mut out = String::new();
    if value.is_sign_negative() {
        out.push('-');
    }
    if (-4..16).contains(&exponent) {
        if exponent >= 0 {
            let point = exponent.unsigned_abs() as usize + 1;
            if digits.len() <= point {
                out.push_str(&digits);
                out.extend(std::iter::repeat_n('0', point - digits.len()));
                out.push_str(".0");
            } else {
                let (whole, fraction) = digits.split_at(point);
                let _ = write!(out, "{whole}.{fraction}");
            }
        } else {
            out.push_str("0.");
            out.extend(std::iter::repeat_n(
                '0',
                exponent.unsigned_abs() as usize - 1,
            ));
            out.push_str(&digits);
        }
    } else {
        let (first, rest) = digits.split_at(1.min(digits.len()));
        out.push_str(first);
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        let _ = write!(
            out,
            "e{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.unsigned_abs()
        );
    }
    Ok(out)
}

/// The shortest digits that read back as `value` (positive, finite) and the exponent of the first,
/// as Python's repr chooses them: of the shortest, the closest, and of two equally close the one
/// whose last digit is even (David Gay's dtoa, mode 0). Rust's `{:e}` prints the shortest and
/// closest too, but breaks that tie upwards.
fn shortest_digits(value: f64) -> (String, i32) {
    let split = |text: &str| -> (String, i32) {
        let (mantissa, exponent) = text.split_once('e').unwrap_or((text, "0"));
        (
            mantissa.chars().filter(char::is_ascii_digit).collect(),
            exponent.parse().unwrap_or(0),
        )
    };
    let (digits, exponent) = split(&format!("{value:e}"));
    let n = digits.len();
    // The exact value, well past those digits: a tie leaves exactly one 5 after the last.
    let (exact, exact_exponent) = split(&format!("{:.*e}", n + 20, value));
    let tail = exact.as_bytes().get(n..).unwrap_or_default();
    let tie = exact_exponent == exponent
        && tail.first() == Some(&b'5')
        && tail.iter().skip(1).all(|d| *d == b'0');
    if !tie {
        return (digits, exponent);
    }
    let low = exact.get(..n).unwrap_or_default();
    if low
        .bytes()
        .last()
        .is_some_and(|d| (d - b'0').is_multiple_of(2))
    {
        return (low.to_owned(), exponent);
    }
    // The upper candidate, one more in the last place; it is even.
    let mut up: Vec<u8> = low.bytes().collect();
    let mut at = up.len();
    while at > 0 {
        at -= 1;
        match up.get_mut(at) {
            Some(digit) if *digit == b'9' => *digit = b'0',
            Some(digit) => {
                *digit += 1;
                return (String::from_utf8(up).unwrap_or_default(), exponent);
            }
            None => break,
        }
    }
    up.insert(0, b'1');
    up.pop();
    (String::from_utf8(up).unwrap_or_default(), exponent + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Int, Object, parse};

    fn written(text: &str, format: Format) -> String {
        write(&parse(text.as_bytes()).unwrap().value, format).unwrap()
    }

    #[test]
    fn a_float_halfway_between_two_shortest_forms_takes_the_even_one() {
        for (text, repr) in [
            ("-1993134990826411.25", "-1993134990826411.2"),
            ("1234567890123456.75", "1234567890123456.8"),
            ("123456789012345.625", "123456789012345.62"),
            ("4503599627370497.5", "4503599627370498.0"),
        ] {
            assert_eq!(float_repr(text.parse().unwrap()).unwrap(), repr, "{text}");
        }
    }

    #[test]
    fn floats_print_as_python_repr() {
        for (value, repr) in [
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (1e-5, "1e-05"),
            (1e-4, "0.0001"),
            (0.1, "0.1"),
            (-0.0, "-0.0"),
            (5e-324, "5e-324"),
            (1.797_693_134_862_315_7e308, "1.7976931348623157e+308"),
            (123_456_789_012_345_680.0, "1.2345678901234568e+17"),
            (1.5e300, "1.5e+300"),
            (100.0, "100.0"),
            (1e22, "1e+22"),
            (0.300_000_000_000_000_04, "0.30000000000000004"),
            (-2.5, "-2.5"),
            (123.456, "123.456"),
        ] {
            assert_eq!(float_repr(value).unwrap(), repr, "{value:e}");
        }
        assert_eq!(float_repr(f64::NAN), Err(WriteError::NotFinite("nan")));
        assert_eq!(
            float_repr(f64::NEG_INFINITY),
            Err(WriteError::NotFinite("-inf"))
        );
    }

    #[test]
    fn strings_escape_as_python_does_in_each_mode() {
        let text = "\u{7f}\u{0}\u{2028}é😀\"\\/\u{8}\u{c}\n\r\t";
        let mut ascii = String::new();
        write_str(&mut ascii, text, true);
        assert_eq!(
            ascii,
            r#""\u007f\u0000\u2028\u00e9\ud83d\ude00\"\\/\b\f\n\r\t""#
        );
        let mut raw = String::new();
        write_str(&mut raw, text, false);
        assert_eq!(raw, "\"\u{7f}\\u0000\u{2028}é😀\\\"\\\\/\\b\\f\\n\\r\\t\"");
    }

    #[test]
    fn every_dialect_lays_out_as_json_dumps() {
        let text = r#"{"b": 1, "a": [1, {}, []], "c": {}}"#;
        assert_eq!(
            written(text, Format::COMPACT),
            r#"{"b":1,"a":[1,{},[]],"c":{}}"#
        );
        assert_eq!(
            written(text, Format::SPACED),
            r#"{"b": 1, "a": [1, {}, []], "c": {}}"#
        );
        assert_eq!(
            written(text, Format::CANONICAL),
            r#"{"a":[1,{},[]],"b":1,"c":{}}"#
        );
        assert_eq!(
            written(text, Format::SORTED_ASCII),
            r#"{"a": [1, {}, []], "b": 1, "c": {}}"#
        );
        assert_eq!(
            written(text, Format::INDENTED),
            "{\n \"b\": 1,\n \"a\": [\n  1,\n  {},\n  []\n ],\n \"c\": {}\n}"
        );
        assert_eq!(written("[]", Format::INDENTED), "[]");
    }

    #[test]
    fn values_built_in_memory_are_checked_before_they_are_written() {
        let mut members = Object::new();
        members.insert("x".into(), Value::Float(f64::INFINITY));
        assert_eq!(
            write(&Value::from(members), Format::COMPACT),
            Err(WriteError::NotFinite("inf"))
        );
        let widest = Int::parse(&"9".repeat(MAX_DIGITS)).unwrap();
        assert!(write(&Value::Int(widest.clone()), Format::COMPACT).is_ok());
        assert_eq!(
            write(&Value::Int(widest.plus_one()), Format::COMPACT),
            Err(WriteError::IntDigits)
        );
        let mut deep = Value::Null;
        for _ in 0..=WRITE_DEPTH {
            deep = Value::Array(vec![deep]);
        }
        assert_eq!(write(&deep, Format::COMPACT), Err(WriteError::TooDeep));
    }
}
