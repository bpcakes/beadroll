//! Integers as Python holds them: exact at any size, within the 4,300 decimal digits Python 3.11 and
//! later read and write by default (`docs/RUST-PORT.md` §2.3a, §5 row 2).

use std::cmp::Ordering;
use std::fmt;

/// The most decimal digits an integer may have, a minus sign not counted: Python's default
/// (`sys.int_info.default_max_str_digits`). A longer integer token is unreadable JSON, and a writer
/// refuses one built in memory.
pub const MAX_DIGITS: usize = 4300;

/// An integer, exact whatever its size. Within `i64` it is a machine integer; beyond, its sign and
/// decimal digits without leading zeros, so that every value has one representation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Int(Repr);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Repr {
    Small(i64),
    Big { negative: bool, digits: Box<str> },
}

impl Int {
    /// Reads a decimal integer: an optional `-`, then ASCII digits. `None` if the text is not one,
    /// or has more than [`MAX_DIGITS`] digits.
    pub fn parse(text: &str) -> Option<Int> {
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        if digits.is_empty()
            || digits.len() > MAX_DIGITS
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        Some(Int::from_parts(negative, digits))
    }

    /// The integer of `negative` and decimal `digits` (ASCII, possibly with leading zeros), without
    /// a length check.
    fn from_parts(negative: bool, digits: &str) -> Int {
        let digits = digits.trim_start_matches('0');
        if digits.is_empty() {
            return Int(Repr::Small(0));
        }
        let small = if digits.len() <= 19 {
            let magnitude = digits.parse::<i128>().ok();
            magnitude
                .map(|m| if negative { -m } else { m })
                .and_then(|m| i64::try_from(m).ok())
        } else {
            None
        };
        match small {
            Some(value) => Int(Repr::Small(value)),
            None => Int(Repr::Big {
                negative,
                digits: digits.into(),
            }),
        }
    }

    /// The value, if it fits an `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        match &self.0 {
            Repr::Small(value) => Some(*value),
            Repr::Big { .. } => None,
        }
    }

    /// How many decimal digits it has, a minus sign not counted.
    pub fn digit_count(&self) -> usize {
        match &self.0 {
            Repr::Small(value) => value
                .unsigned_abs()
                .checked_ilog10()
                .map_or(1, |log| log as usize + 1),
            Repr::Big { digits, .. } => digits.len(),
        }
    }

    /// Whether it is below zero.
    pub fn is_negative(&self) -> bool {
        match &self.0 {
            Repr::Small(value) => *value < 0,
            Repr::Big { negative, .. } => *negative,
        }
    }

    /// This integer plus one, as Python's `n + 1`.
    pub fn plus_one(&self) -> Int {
        if let Repr::Small(value) = &self.0
            && let Some(next) = value.checked_add(1)
        {
            return Int(Repr::Small(next));
        }
        let (negative, digits) = self.sign_and_digits();
        if negative {
            Int::from_parts(true, &decrement(&digits))
        } else {
            Int::from_parts(false, &increment(&digits))
        }
    }

    fn sign_and_digits(&self) -> (bool, String) {
        match &self.0 {
            Repr::Small(value) => (*value < 0, value.unsigned_abs().to_string()),
            Repr::Big { negative, digits } => (*negative, digits.to_string()),
        }
    }
}

impl From<i64> for Int {
    fn from(value: i64) -> Int {
        Int(Repr::Small(value))
    }
}

/// `digits` plus one: decimal, no leading zeros.
fn increment(digits: &str) -> String {
    let mut bytes = digits.as_bytes().to_vec();
    for byte in bytes.iter_mut().rev() {
        if *byte == b'9' {
            *byte = b'0';
        } else {
            *byte += 1;
            return String::from_utf8_lossy(&bytes).into_owned();
        }
    }
    let mut out = String::with_capacity(bytes.len() + 1);
    out.push('1');
    out.push_str(&String::from_utf8_lossy(&bytes));
    out
}

/// `digits` minus one, for a positive magnitude.
fn decrement(digits: &str) -> String {
    let mut bytes = digits.as_bytes().to_vec();
    for byte in bytes.iter_mut().rev() {
        if *byte == b'0' {
            *byte = b'9';
        } else {
            *byte -= 1;
            break;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

impl Ord for Int {
    fn cmp(&self, other: &Int) -> Ordering {
        if let (Repr::Small(a), Repr::Small(b)) = (&self.0, &other.0) {
            return a.cmp(b);
        }
        let (a_negative, a_digits) = self.sign_and_digits();
        let (b_negative, b_digits) = other.sign_and_digits();
        let magnitude = a_digits
            .len()
            .cmp(&b_digits.len())
            .then_with(|| a_digits.cmp(&b_digits));
        match (a_negative, b_negative) {
            (false, false) => magnitude,
            (true, true) => magnitude.reverse(),
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
        }
    }
}

impl PartialOrd for Int {
    fn partial_cmp(&self, other: &Int) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Int {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Repr::Small(value) => write!(f, "{value}"),
            Repr::Big { negative, digits } => {
                write!(f, "{}{digits}", if *negative { "-" } else { "" })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(text: &str) -> Int {
        Int::parse(text).unwrap()
    }

    #[test]
    fn integers_read_as_python_reads_them() {
        assert_eq!(int("-0"), Int::from(0));
        assert_eq!(int("-0").to_string(), "0");
        assert_eq!(int("9223372036854775807").as_i64(), Some(i64::MAX));
        assert_eq!(int("-9223372036854775808").as_i64(), Some(i64::MIN));
        assert_eq!(int("9223372036854775808").as_i64(), None);
        assert_eq!(
            int("9223372036854775808").to_string(),
            "9223372036854775808"
        );
        let wide = "1".repeat(MAX_DIGITS);
        assert_eq!(int(&wide).to_string(), wide);
        assert_eq!(int(&format!("-{wide}")).digit_count(), MAX_DIGITS);
        assert!(Int::parse(&"1".repeat(MAX_DIGITS + 1)).is_none());
        assert!(Int::parse(&format!("-{}", "1".repeat(MAX_DIGITS + 1))).is_none());
        for bad in ["", "-", "+1", "1.0", "1e3", "٣", " 1"] {
            assert!(Int::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn plus_one_carries_across_every_size() {
        assert_eq!(
            Int::from(i64::MAX).plus_one().to_string(),
            "9223372036854775808"
        );
        assert_eq!(
            int("-9223372036854775809").plus_one().as_i64(),
            Some(i64::MIN)
        );
        assert_eq!(
            int("99999999999999999999").plus_one().to_string(),
            "100000000000000000000"
        );
        assert_eq!(
            int("-100000000000000000000").plus_one().to_string(),
            "-99999999999999999999"
        );
        assert_eq!(Int::from(-1).plus_one(), Int::from(0));
        assert_eq!(
            int(&"9".repeat(MAX_DIGITS)).plus_one().digit_count(),
            MAX_DIGITS + 1
        );
    }

    #[test]
    fn ordering_is_numeric() {
        let mut values: Vec<Int> = [
            "5",
            "-99999999999999999999",
            "99999999999999999999",
            "0",
            "-3",
            "100000000000000000000",
        ]
        .into_iter()
        .map(int)
        .collect();
        values.sort();
        let shown: Vec<String> = values.iter().map(Int::to_string).collect();
        assert_eq!(
            shown,
            [
                "-99999999999999999999",
                "-3",
                "0",
                "5",
                "99999999999999999999",
                "100000000000000000000"
            ]
        );
        assert_eq!(Int::from(i64::MIN).digit_count(), 19);
        assert_eq!(Int::from(0).digit_count(), 1);
    }
}
