//! Numeric text accepted by `--limit`, `--estimate` and the `BEAD_*` settings.
//!
//! Non-ASCII whitespace becomes a space and each decimal digit (Unicode category Nd) its ASCII
//! digit; anything else non-ASCII makes the string unreadable. ASCII characters stay as they are,
//! so `\x1c` is not whitespace here. Then the ASCII whitespace at either end goes, and an
//! underscore may stand only between two digits. These rules preserve the original command-line
//! grammar (`docs/RUST-PORT.md` §5 row 12).

use crate::json::Int;
use crate::text::is_space;

/// The first code point of each run of ten decimal digits in Unicode 16.0, starting with ASCII.
const DECIMAL_RUNS: [u32; 76] = [
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66,
    0xDE6, 0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    0x104A0, 0x10D30, 0x10D40, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0,
    0x11650, 0x116C0, 0x116D0, 0x116DA, 0x11730, 0x118E0, 0x11950, 0x11BF0, 0x11C50, 0x11D50,
    0x11DA0, 0x11F50, 0x16130, 0x16A60, 0x16AC0, 0x16B50, 0x16D70, 0x1CCF0, 0x1D7CE, 0x1D7D8,
    0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E4F0, 0x1E5F1, 0x1E950, 0x1FBF0,
];

/// The characters that are digits but not decimal digits (Unicode `Numeric_Type=Digit`: the
/// superscripts, circled and parenthesised digits and the like), as ranges: [`is_digits`] takes
/// them, [`parse_integer`] does not.
const DIGIT_ONLY: [(u32, u32); 20] = [
    (0xB2, 0xB3),
    (0xB9, 0xB9),
    (0x1369, 0x1371),
    (0x19DA, 0x19DA),
    (0x2070, 0x2070),
    (0x2074, 0x2079),
    (0x2080, 0x2089),
    (0x2460, 0x2468),
    (0x2474, 0x247C),
    (0x2488, 0x2490),
    (0x24EA, 0x24EA),
    (0x24F5, 0x24FD),
    (0x24FF, 0x24FF),
    (0x2776, 0x277E),
    (0x2780, 0x2788),
    (0x278A, 0x2792),
    (0x10A40, 0x10A43),
    (0x10E60, 0x10E68),
    (0x11052, 0x1105A),
    (0x1F100, 0x1F10A),
];

/// Whether `text` has at least one character and every character is a Unicode digit (decimal or
/// not). Non-decimal digits do not necessarily form a readable integer.
pub fn is_digits(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| {
            let code = u32::from(c);
            decimal_value(c).is_some()
                || DIGIT_ONLY
                    .iter()
                    .any(|(start, end)| (*start..=*end).contains(&code))
        })
}

/// The value of `c` as a Unicode decimal digit.
pub fn decimal_value(c: char) -> Option<u8> {
    let code = u32::from(c);
    let at = DECIMAL_RUNS.partition_point(|start| *start <= code);
    let start = DECIMAL_RUNS.get(at.checked_sub(1)?)?;
    u8::try_from(code - start).ok().filter(|digit| *digit < 10)
}

/// Normalises decimal digits and non-ASCII whitespace, then trims ASCII whitespace at either end.
/// Returns `None` for any other non-ASCII character.
fn ascii_form(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if u32::from(c) < 127 {
            out.push(c);
        } else if is_space(c) {
            out.push(' ');
        } else {
            out.push(char::from(b'0' + decimal_value(c)?));
        }
    }
    let ascii_space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{0b}' | '\u{0c}');
    Some(out.trim_matches(ascii_space).to_owned())
}

/// The text without its underscores, when each stands between two digits.
fn without_underscores(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut previous = '\0';
    for c in text.chars() {
        if c == '_' {
            if !previous.is_ascii_digit() {
                return None;
            }
        } else {
            if previous == '_' && !c.is_ascii_digit() {
                return None;
            }
            out.push(c);
        }
        previous = c;
    }
    (previous != '_').then_some(out)
}

/// An exact decimal integer with an optional sign, Unicode decimal digits, underscores between
/// them and whitespace around them. Refused beyond 4,300 digits (leading zeros counted,
/// underscores not).
pub fn parse_integer(text: &str) -> Option<Int> {
    let ascii = ascii_form(text)?;
    let (sign, digits) = match ascii.as_bytes().first() {
        Some(b'-') => ("-", &ascii[1..]),
        Some(b'+') => ("", &ascii[1..]),
        _ => ("", ascii.as_str()),
    };
    if !digits.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let digits = without_underscores(digits)?;
    // Int::parse refuses anything but ASCII digits, and more than MAX_DIGITS of them.
    Int::parse(&format!("{sign}{digits}"))
}

/// A decimal number, `inf`, `infinity` or `nan` in any case, with an optional sign, Unicode decimal
/// digits, underscores between digits and whitespace around it. Overflow produces infinity.
pub fn parse_float(text: &str) -> Option<f64> {
    let ascii = ascii_form(text)?;
    let plain = if ascii.contains('_') {
        without_underscores(&ascii)?
    } else {
        ascii
    };
    let body = plain.trim_start_matches(['+', '-']);
    if plain.len() - body.len() > 1 {
        return None; // one sign at most
    }
    let word = body.to_ascii_lowercase();
    if !matches!(word.as_str(), "inf" | "infinity" | "nan") {
        // Only decimal syntax reaches the parser here; special values were handled above.
        let shaped = body
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'));
        if !shaped || body.is_empty() {
            return None;
        }
    }
    plain.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_accept_unicode_decimal_digits_and_separators() {
        let cases: [(&str, Option<i64>); 18] = [
            (" \u{663} ", Some(3)),
            ("1_000", Some(1000)),
            ("+5", Some(5)),
            ("-0", Some(0)),
            ("\u{ff11}\u{ff12}", Some(12)),
            ("007", Some(7)),
            (" 1 2", None),
            ("_1", None),
            ("1_", None),
            ("1__0", None),
            ("0x10", None),
            ("", None),
            ("\u{1c}7\u{1f}", None),
            ("1.0", None),
            ("\u{2000}7", Some(7)),
            ("\u{1c}7\u{2000}", None),
            ("7\u{85}", Some(7)),
            ("+-1", None),
        ];
        for (text, expected) in cases {
            assert_eq!(parse_integer(text), expected.map(Int::from), "{text:?}");
        }
        assert_eq!(
            parse_integer(&"9".repeat(50)).unwrap().to_string(),
            "9".repeat(50)
        );
        // The limit counts leading zeros, but not underscores.
        assert!(parse_integer(&"1".repeat(4300)).is_some());
        assert!(parse_integer(&"1".repeat(4301)).is_none());
        assert!(parse_integer(&format!("{}1", "0".repeat(4300))).is_none());
        assert!(parse_integer(&format!("-{}", "1".repeat(4301))).is_none());
        assert!(parse_integer(&format!("{}1", "1_".repeat(2150))).is_some());
        assert!(parse_integer(&"\u{661}".repeat(4301)).is_none());
    }

    #[test]
    fn floats_accept_decimal_syntax_and_special_values() {
        let cases: [(&str, Option<f64>); 17] = [
            ("1_000.5", Some(1000.5)),
            (" inf ", Some(f64::INFINITY)),
            ("-Infinity", Some(f64::NEG_INFINITY)),
            ("1e3", Some(1000.0)),
            (".5", Some(0.5)),
            ("5.", Some(5.0)),
            ("1_e3", None),
            ("\u{661}.\u{665}", Some(1.5)),
            ("1e400", Some(f64::INFINITY)),
            ("+.5e-1_0", Some(5e-11)),
            ("1.5_", None),
            ("in f", None),
            ("infinit", None),
            ("1e", None),
            ("e3", None),
            (".", None),
            ("-", None),
        ];
        for (text, expected) in cases {
            assert_eq!(parse_float(text), expected, "{text:?}");
        }
        assert!(parse_float("NaN ").is_some_and(f64::is_nan));
        assert_eq!(parse_float("\u{1c}1.5"), None);
        assert_eq!(parse_float("\u{a0}inf"), Some(f64::INFINITY));
    }

    #[test]
    fn digits_include_non_decimal_unicode_digits() {
        assert!(is_digits("123") && is_digits("\u{663}") && is_digits("\u{b2}"));
        assert!(!is_digits("") && !is_digits(" 1") && !is_digits("1.0"));
        assert_eq!(parse_integer("\u{b2}"), None);
    }

    #[test]
    fn decimal_digits_have_their_values() {
        assert_eq!(decimal_value('7'), Some(7));
        assert_eq!(decimal_value('\u{669}'), Some(9));
        assert_eq!(decimal_value('\u{1fbf9}'), Some(9));
        assert_eq!(decimal_value('a'), None);
        assert_eq!(decimal_value('\u{65f}'), None);
    }
}
