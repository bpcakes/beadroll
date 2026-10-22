//! Exact integers within an `i128` (roll-lqjj.53.2): what bead reads (an integer within a `u64` or
//! an `i64`, which is what serde_json keeps exact; one beyond arrives as a float), counts (a
//! segment's number, a claim's generation, the unpublished count) and mints (a key's number). Each
//! count is checked, never wrapped: what cannot be counted on is `None`, and the caller says so.
//! The 4,300-digit decimal limit of Python 3.11's `json`, kept until then for parity with the
//! Python bead, is gone with the parity.

use std::fmt;

/// An integer, exact within an `i128`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Int(i128);

impl Int {
    /// Reads a decimal integer: an optional `-`, then ASCII digits. `None` if the text is not one,
    /// or its value is beyond an `i128`.
    pub fn parse(text: &str) -> Option<Int> {
        let digits = text.strip_prefix('-').unwrap_or(text);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok().map(Int)
    }

    /// The integer `value` holds.
    pub fn from_u64(value: u64) -> Int {
        Int(i128::from(value))
    }

    /// The integer `value` holds.
    pub fn from_i128(value: i128) -> Int {
        Int(value)
    }

    /// The value, if it fits an `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        i64::try_from(self.0).ok()
    }

    /// The value.
    pub fn as_i128(&self) -> i128 {
        self.0
    }

    /// This integer plus one, or `None` beyond an `i128`.
    pub fn plus_one(&self) -> Option<Int> {
        self.0.checked_add(1).map(Int)
    }

    /// This integer plus `other`, or `None` beyond an `i128`.
    pub fn add(&self, other: &Int) -> Option<Int> {
        self.0.checked_add(other.0).map(Int)
    }

    /// This integer minus `other`, or `None` beyond an `i128`.
    pub fn sub(&self, other: &Int) -> Option<Int> {
        self.0.checked_sub(other.0).map(Int)
    }

    /// The integer part of the finite float `value`, truncated toward zero. `None` for NaN,
    /// infinity, or a value beyond an `i128`.
    pub fn from_f64(value: f64) -> Option<Int> {
        let whole = value.trunc();
        // Every float from -2^127 up to but not including 2^127 is an i128, exactly.
        (whole.is_finite() && whole >= -(2f64.powi(127)) && whole < 2f64.powi(127))
            .then_some(Int(whole as i128))
    }

    /// Digits zero-padded after the sign to a total width of `width` characters.
    pub fn zero_padded(&self, width: usize) -> String {
        let sign = if self.0 < 0 { "-" } else { "" };
        let room = width.saturating_sub(sign.len());
        format!("{sign}{:0>room$}", self.0.unsigned_abs())
    }
}

impl From<i64> for Int {
    fn from(value: i64) -> Int {
        Int(i128::from(value))
    }
}

impl fmt::Display for Int {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(text: &str) -> Int {
        Int::parse(text).unwrap()
    }

    const MAX: &str = "170141183460469231731687303715884105727";
    const MIN: &str = "-170141183460469231731687303715884105728";

    #[test]
    fn integers_parse_exactly_within_an_i128() {
        assert_eq!(int("-0"), Int::from(0));
        assert_eq!(int("-0").to_string(), "0");
        assert_eq!(int("007"), Int::from(7));
        assert_eq!(int("9223372036854775807").as_i64(), Some(i64::MAX));
        assert_eq!(int("-9223372036854775808").as_i64(), Some(i64::MIN));
        assert_eq!(int("9223372036854775808").as_i64(), None);
        assert_eq!(
            int("9223372036854775808").to_string(),
            "9223372036854775808"
        );
        assert_eq!(int("18446744073709551615"), Int::from_u64(u64::MAX));
        assert_eq!(int(MAX).as_i128(), i128::MAX);
        assert_eq!(int(MIN).as_i128(), i128::MIN);
        assert_eq!(int(MAX).to_string(), MAX);
        assert_eq!(int(MIN).to_string(), MIN);
        assert!(Int::parse("170141183460469231731687303715884105728").is_none());
        assert!(Int::parse("-170141183460469231731687303715884105729").is_none());
        assert!(Int::parse(&"1".repeat(40)).is_none());
        for bad in ["", "-", "+1", "1.0", "1e3", "٣", " 1", "1_0", "--1"] {
            assert!(Int::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn counting_is_checked_at_the_edges() {
        assert_eq!(
            Int::from(i64::MAX).plus_one().unwrap().to_string(),
            "9223372036854775808"
        );
        assert_eq!(Int::from(-1).plus_one(), Some(Int::from(0)));
        assert_eq!(int(MAX).plus_one(), None);
        assert_eq!(
            Int::from(i64::MAX).add(&Int::from(1)).unwrap().to_string(),
            "9223372036854775808"
        );
        assert_eq!(
            Int::from(5).sub(&Int::from(i64::MIN)).unwrap().to_string(),
            "9223372036854775813"
        );
        assert_eq!(
            int("100000000000000000000").sub(&int("100000000000000000001")),
            Some(Int::from(-1))
        );
        assert_eq!(Int::from(-7).sub(&Int::from(-7)), Some(Int::from(0)));
        assert_eq!(int(MAX).add(&Int::from(1)), None);
        assert_eq!(int(MIN).sub(&Int::from(1)), None);
        assert_eq!(int(MIN).add(&int(MAX)), Some(Int::from(-1)));
    }

    #[test]
    fn floats_truncate_exactly_within_an_i128() {
        assert_eq!(Int::from_f64(-1.5), Some(Int::from(-1)));
        assert_eq!(Int::from_f64(0.9), Some(Int::from(0)));
        assert_eq!(
            Int::from_f64(2f64.powi(70)).unwrap().to_string(),
            "1180591620717411303424"
        );
        assert_eq!(
            Int::from_f64(-(2f64.powi(127))).map(|n| n.as_i128()),
            Some(i128::MIN)
        );
        assert_eq!(Int::from_f64(2f64.powi(127)), None);
        assert_eq!(Int::from_f64(1e300), None);
        assert_eq!(Int::from_f64(f64::NAN), None);
        assert_eq!(Int::from_f64(f64::INFINITY), None);
    }

    #[test]
    fn padding_keeps_the_sign_first() {
        assert_eq!(Int::from(7).zero_padded(6), "000007");
        assert_eq!(Int::from(-4).zero_padded(6), "-00004");
        assert_eq!(Int::from(1234567).zero_padded(6), "1234567");
        assert_eq!(int(MIN).zero_padded(6), MIN);
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
            MIN,
            MAX,
        ]
        .into_iter()
        .map(int)
        .collect();
        values.sort();
        let shown: Vec<String> = values.iter().map(Int::to_string).collect();
        assert_eq!(
            shown,
            [
                MIN,
                "-99999999999999999999",
                "-3",
                "0",
                "5",
                "99999999999999999999",
                "100000000000000000000",
                MAX,
            ]
        );
    }
}
