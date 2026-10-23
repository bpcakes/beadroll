//! Numeric text accepted by `--limit`, `--estimate` and the `BEAD_*` settings (roll-lqjj.53.2):
//! what Rust reads, whitespace around it allowed. An integer is an optional sign and ASCII digits
//! within an `i128`; a float is what `f64` parses (`1e3`, `.5`, `5.`, `inf`, `nan`). Until
//! roll-lqjj.53.2 these read as Python's `int()` and `float()` did, for parity with the Python
//! bead (`" +5 "`, `5_000`, digits of every script), and that parity is gone.

use crate::json::Int;

/// Whether `text` has at least one character and every character is an ASCII digit.
pub fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// An exact decimal integer with an optional sign and whitespace around it, within an `i128`.
pub fn parse_integer(text: &str) -> Option<Int> {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if !is_digits(digits) {
        return None;
    }
    trimmed.parse().ok().map(Int::from_i128)
}

/// A decimal number, `inf`, `infinity` or `nan` in any case, with an optional sign and whitespace
/// around it. Overflow produces infinity.
pub fn parse_float(text: &str) -> Option<f64> {
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_signed_ascii_digits_with_whitespace_around() {
        let cases: [(&str, Option<i64>); 16] = [
            (" 3 ", Some(3)),
            ("+5", Some(5)),
            ("-0", Some(0)),
            ("007", Some(7)),
            ("\t12\n", Some(12)),
            ("\u{2000}7", Some(7)),
            ("1_000", None),
            (" 1 2", None),
            ("_1", None),
            ("0x10", None),
            ("", None),
            ("\u{1c}7\u{1f}", None),
            ("1.0", None),
            ("+-1", None),
            ("\u{663}", None),
            ("\u{ff11}\u{ff12}", None),
        ];
        for (text, expected) in cases {
            assert_eq!(parse_integer(text), expected.map(Int::from), "{text:?}");
        }
        let most = i128::MAX.to_string();
        assert_eq!(parse_integer(&most).map(|n| n.as_i128()), Some(i128::MAX));
        assert_eq!(
            parse_integer(&i128::MIN.to_string()).map(|n| n.as_i128()),
            Some(i128::MIN)
        );
        assert!(parse_integer(&"9".repeat(40)).is_none());
        assert!(parse_integer(&format!("-{}", "9".repeat(40))).is_none());
    }

    #[test]
    fn floats_are_what_f64_parses_with_whitespace_around() {
        let cases: [(&str, Option<f64>); 16] = [
            ("1000.5", Some(1000.5)),
            (" inf ", Some(f64::INFINITY)),
            ("-Infinity", Some(f64::NEG_INFINITY)),
            ("1e3", Some(1000.0)),
            (".5", Some(0.5)),
            ("5.", Some(5.0)),
            ("1e400", Some(f64::INFINITY)),
            ("+.5e-10", Some(5e-11)),
            ("1_000.5", None),
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
        assert_eq!(parse_float("\u{663}.5"), None);
    }

    #[test]
    fn digits_are_ascii_digits() {
        assert!(is_digits("123") && is_digits("0"));
        assert!(!is_digits("") && !is_digits(" 1") && !is_digits("1.0"));
        assert!(!is_digits("\u{663}") && !is_digits("\u{b2}"));
    }
}
