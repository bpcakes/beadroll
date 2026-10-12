//! Exact integers with a 4,300-digit decimal read/write limit (`docs/RUST-PORT.md` §2.3a, §5 row
//! 2).

use std::cmp::Ordering;
use std::fmt;

/// Maximum decimal digits, excluding the minus sign. Longer JSON tokens are unreadable; writers
/// also refuse oversized integers constructed in memory.
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

    /// This integer plus one, exactly.
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

    /// This integer plus `other`, exactly.
    pub fn add(&self, other: &Int) -> Int {
        if let (Repr::Small(a), Repr::Small(b)) = (&self.0, &other.0)
            && let Some(sum) = a.checked_add(*b)
        {
            return Int(Repr::Small(sum));
        }
        let (a_negative, a_digits) = self.sign_and_digits();
        let (b_negative, b_digits) = other.sign_and_digits();
        if a_negative == b_negative {
            return Int::from_parts(a_negative, &add_digits(&a_digits, &b_digits));
        }
        match compare_digits(&a_digits, &b_digits) {
            Ordering::Equal => Int(Repr::Small(0)),
            Ordering::Greater => {
                Int::from_parts(a_negative, &subtract_digits(&a_digits, &b_digits))
            }
            Ordering::Less => Int::from_parts(b_negative, &subtract_digits(&b_digits, &a_digits)),
        }
    }

    /// This integer minus `other`, exactly.
    pub fn sub(&self, other: &Int) -> Int {
        let (negative, digits) = other.sign_and_digits();
        self.add(&Int::from_parts(!negative, &digits))
    }

    /// The exact integer part of the finite float `value`, truncated toward zero. Returns `None`
    /// for NaN or infinity.
    pub fn from_f64(value: f64) -> Option<Int> {
        if !value.is_finite() {
            return None;
        }
        let whole = value.trunc();
        if whole.abs() < 2f64.powi(63) {
            return Some(Int(Repr::Small(whole as i64)));
        }
        // An integral float this large is its 53-bit mantissa times a power of two.
        let bits = whole.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i64 - 1075;
        let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
        let mut digits = mantissa.to_string();
        for _ in 0..exponent.max(0) {
            digits = add_digits(&digits, &digits);
        }
        Some(Int::from_parts(whole < 0.0, &digits))
    }

    /// The value in `i128`, or the nearest bound of `i128` beyond it: it compares as the integer
    /// does against anything `i128` holds.
    pub fn saturating_i128(&self) -> i128 {
        match &self.0 {
            Repr::Small(value) => i128::from(*value),
            Repr::Big { negative, digits } => {
                let magnitude = digits.parse::<i128>().ok();
                match (magnitude, negative) {
                    (Some(m), false) => m,
                    (Some(m), true) => -m,
                    (None, false) => i128::MAX,
                    (None, true) => i128::MIN,
                }
            }
        }
    }

    /// Digits zero-padded after the sign to a total width of `width` characters.
    pub fn zero_padded(&self, width: usize) -> String {
        let (negative, digits) = self.sign_and_digits();
        let sign = if negative { "-" } else { "" };
        let room = width.saturating_sub(sign.len());
        format!("{sign}{digits:0>room$}")
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

/// The sum of two decimal magnitudes.
fn add_digits(a: &str, b: &str) -> String {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
    let mut carry = 0u8;
    for at in 0..a.len().max(b.len()) {
        let digit = |x: &[u8]| {
            x.len()
                .checked_sub(at + 1)
                .and_then(|i| x.get(i))
                .map_or(0, |d| d - b'0')
        };
        let total = digit(a) + digit(b) + carry;
        out.push(b'0' + total % 10);
        carry = total / 10;
    }
    if carry > 0 {
        out.push(b'0' + carry);
    }
    out.reverse();
    String::from_utf8_lossy(&out).into_owned()
}

/// The difference of two decimal magnitudes, the first not the smaller.
fn subtract_digits(a: &str, b: &str) -> String {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut out = Vec::with_capacity(a.len());
    let mut borrow = 0i8;
    for at in 0..a.len() {
        let digit = |x: &[u8]| {
            x.len()
                .checked_sub(at + 1)
                .and_then(|i| x.get(i))
                .map_or(0, |d| (d - b'0') as i8)
        };
        let mut value = digit(a) - digit(b) - borrow;
        borrow = 0;
        if value < 0 {
            value += 10;
            borrow = 1;
        }
        out.push(b'0' + value as u8);
    }
    out.reverse();
    String::from_utf8_lossy(&out).into_owned()
}

/// How two decimal magnitudes without leading zeros compare.
fn compare_digits(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
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
    fn arithmetic_is_exact_at_every_size() {
        assert_eq!(
            Int::from(i64::MAX).add(&Int::from(1)).to_string(),
            "9223372036854775808"
        );
        assert_eq!(
            Int::from(5).sub(&Int::from(i64::MIN)).to_string(),
            "9223372036854775813"
        );
        assert_eq!(
            int("100000000000000000000").sub(&int("100000000000000000001")),
            Int::from(-1)
        );
        assert_eq!(
            int("-99999999999999999999").add(&int("99999999999999999999")),
            Int::from(0)
        );
        assert_eq!(Int::from(-7).sub(&Int::from(-7)), Int::from(0));
    }

    #[test]
    fn floats_truncate_exactly() {
        // Exact truncations of 1e300, -1.5 and 2 raised to the power 70.
        let big = "1000000000000000052504760255204420248704468581108159154915854115511802457988908195786371375080447864043704443832883878176942523235360430575644792184786706982848387200926575803737830233794788090059368953234970799945081119038967640880074652742780142494579258788820056842838115669472196386865459400540160";
        assert_eq!(Int::from_f64(1e300).unwrap().to_string(), big);
        assert_eq!(Int::from_f64(-1.5), Some(Int::from(-1)));
        assert_eq!(
            Int::from_f64(2f64.powi(70)).unwrap().to_string(),
            "1180591620717411303424"
        );
        assert_eq!(Int::from_f64(f64::NAN), None);
    }

    #[test]
    fn saturation_and_padding() {
        assert_eq!(int(&"9".repeat(50)).saturating_i128(), i128::MAX);
        assert_eq!(
            int(&format!("-{}", "9".repeat(50))).saturating_i128(),
            i128::MIN
        );
        assert_eq!(Int::from(-4).saturating_i128(), -4);
        assert_eq!(Int::from(7).zero_padded(6), "000007");
        assert_eq!(Int::from(-4).zero_padded(6), "-00004");
        assert_eq!(Int::from(1234567).zero_padded(6), "1234567");
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
