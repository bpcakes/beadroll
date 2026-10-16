//! Seeded JSON values for the round-trip properties (roll-k2wy.14.4): strings of every kind of
//! character a writer escapes or passes through (controls, quotes, backslashes, DEL, the line
//! separators JSON allows raw, the private-use characters that stand in for lone surrogates, a
//! BOM, characters beyond the BMP), integers up to [`MAX_DIGITS`] digits, floats of every bit
//! pattern that is finite, and arrays and objects nested to a given depth; and [`identical`], the
//! comparison that tells `0.0` from `-0.0` and one key order from another, which `Value`'s
//! equality does not. A failure names its seed: the same seed draws the same values on every run.

use beadroll_core::json::{Int, MAX_DIGITS, Object};

use crate::Json;

/// A seeded generator (SplitMix64): the same draws on every run.
pub struct Rng(u64);

/// Characters a writer treats each its own way, drawn often among the rest.
const AWKWARD: [char; 22] = [
    '"',
    '\\',
    '/',
    '\n',
    '\r',
    '\t',
    '\u{8}',
    '\u{c}',
    '\u{0}',
    '\u{1f}',
    '\u{7f}',
    '\u{80}',
    '\u{85}',
    '\u{a0}',
    '\u{2028}',
    '\u{2029}',
    '\u{e000}',
    '\u{f8ff}',
    '\u{feff}',
    '\u{fffd}',
    '\u{1f600}',
    '\u{10ffff}',
];

/// Floats at the edges of how they are written: zeros, the bounds of positional notation, the
/// smallest and largest, and halves that round to an even last digit.
const EDGES: [f64; 13] = [
    0.0,
    -0.0,
    1e16,
    9_999_999_999_999_998.0,
    1e-4,
    f64::from_bits(1e-4f64.to_bits() - 1),
    5e-324,
    f64::MIN_POSITIVE,
    f64::MAX,
    f64::MIN,
    1.0,
    -2.5,
    123_456_789_012_345.62,
];

impl Rng {
    /// The generator of `seed`.
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// The next 64 bits.
    pub fn bits(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number below `n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        (self.bits() % n.max(1) as u64) as usize
    }

    /// True `percent` times in a hundred.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.bits() % 100 < percent
    }

    /// A character: printable ASCII half the time, else one of [`AWKWARD`] or any Unicode scalar.
    pub fn char(&mut self) -> char {
        match self.below(4) {
            0 | 1 => char::from(b' ' + self.below(95) as u8),
            2 => AWKWARD[self.below(AWKWARD.len())],
            _ => loop {
                if let Some(c) = char::from_u32((self.bits() % 0x11_0000) as u32) {
                    break c;
                }
            },
        }
    }

    /// A string of up to `most` characters.
    pub fn text(&mut self, most: usize) -> String {
        let length = self.below(most + 1);
        (0..length).map(|_| self.char()).collect()
    }

    /// An integer: small, an `i64` at its edges, or up to [`MAX_DIGITS`] digits.
    pub fn int(&mut self) -> Int {
        match self.below(6) {
            0 => Int::from(i64::MIN),
            1 => Int::from(i64::MAX),
            2 => {
                let most = if self.chance(10) { MAX_DIGITS } else { 40 };
                let digits = 1 + self.below(most);
                let mut text = String::with_capacity(digits + 1);
                if self.chance(50) {
                    text.push('-');
                }
                text.push(char::from(b'1' + self.below(9) as u8));
                for _ in 1..digits {
                    text.push(char::from(b'0' + self.below(10) as u8));
                }
                Int::parse(&text).expect("digits are an integer")
            }
            _ => Int::from(self.bits() as i64 >> self.below(64)),
        }
    }

    /// A finite float: any bit pattern, or one of [`EDGES`], or a whole number.
    pub fn float(&mut self) -> f64 {
        match self.below(4) {
            0 => EDGES[self.below(EDGES.len())],
            1 => (self.bits() as i64 >> self.below(64)) as f64,
            _ => loop {
                let float = f64::from_bits(self.bits());
                if float.is_finite() {
                    break float;
                }
            },
        }
    }

    /// A value nested at most `depth` deep: a scalar at depth 0.
    pub fn value(&mut self, depth: usize) -> Json {
        let kinds = if depth == 0 { 6 } else { 8 };
        match self.below(kinds) {
            0 => Json::Null,
            1 => Json::Bool(self.chance(50)),
            2 => Json::Int(self.int()),
            3 => Json::Float(self.float()),
            4 | 5 => Json::from(self.text(12)),
            6 => {
                let length = self.below(5);
                Json::Array((0..length).map(|_| self.value(depth - 1)).collect())
            }
            _ => Json::from(self.object(depth - 1)),
        }
    }

    /// An object of up to four members, each nested at most `depth` deep.
    pub fn object(&mut self, depth: usize) -> Object {
        let length = self.below(5);
        (0..length)
            .map(|_| (self.text(6), self.value(depth)))
            .collect()
    }

    /// `items` in an order drawn at random.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for at in (1..items.len()).rev() {
            items.swap(at, self.below(at + 1));
        }
    }
}

/// Whether `a` and `b` are the same value member for member, in the same order, every float the
/// same bits: what reading back a value written in a dialect that keeps key order must give.
pub fn identical(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Float(a), Json::Float(b)) => a.to_bits() == b.to_bits(),
        (Json::Array(a), Json::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| identical(a, b))
        }
        (Json::Object(a), Json::Object(b)) => {
            a.len() == b.len()
                && (a.iter().zip(b.iter()))
                    .all(|((ka, va), (kb, vb))| ka == kb && identical(va, vb))
        }
        (a, b) => a == b,
    }
}

/// `value` with every object's members in key order, as a dialect that sorts keys writes it.
pub fn sorted(value: &Json) -> Json {
    match value {
        Json::Array(items) => Json::Array(items.iter().map(sorted).collect()),
        Json::Object(members) => {
            let mut members: Vec<(&String, &Json)> = members.iter().collect();
            members.sort_by(|a, b| a.0.cmp(b.0));
            Json::from(
                (members.into_iter())
                    .map(|(key, value)| (key.clone(), sorted(value)))
                    .collect::<Object>(),
            )
        }
        other => other.clone(),
    }
}
