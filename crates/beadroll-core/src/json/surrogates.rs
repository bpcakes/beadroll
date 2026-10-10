//! Lone surrogates kept through a read and a write. Python's `json.loads` keeps a lone surrogate as
//! a character of the string it reads (a `str` may hold one, as `surrogateescape` also makes them of
//! bytes that are not UTF-8), and `json.dumps` spells it again as `\udXXX`; [`super::parse()`] reads
//! one as U+FFFD, as bead's journal readers do. A file Python rewrites whole (`bead machines --name`
//! rewrites `aliases.json`) is read here instead: each lone surrogate stands as a private-use
//! character the text holds nowhere else, so parsing keeps the surrogates apart and a write can
//! spell them back ([`Surrogates::restore`]).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

/// Each `\uXXXX` escape in the string literals of `text` that spells a lone surrogate, with its code
/// unit: a high surrogate not followed by an escaped low one, or a low surrogate not preceded by a
/// high one. An escaped pair is one character, as Python's `json` reads it. Text that is not JSON
/// gives what its string literals hold so far; [`super::parse()`] says what is wrong with it.
pub fn lone_surrogate_escapes(text: &str) -> Vec<(Range<usize>, u16)> {
    let mut out = Vec::new();
    for_each_escape(text, |at, unit, lone| {
        if lone {
            out.push((at, unit));
        }
    });
    out
}

/// Calls `found` with each `\uXXXX` escape in the string literals of `text`: where it is, its code
/// unit, and whether it is a lone surrogate. An escaped surrogate pair is reported as its two halves,
/// neither lone.
fn for_each_escape(text: &str, mut found: impl FnMut(Range<usize>, u16, bool)) {
    let bytes = text.as_bytes();
    let unit_at = |at: usize| -> Option<u16> {
        let escape = bytes.get(at..at + 6)?;
        if !escape.starts_with(b"\\u") {
            return None;
        }
        u16::from_str_radix(std::str::from_utf8(&escape[2..]).ok()?, 16).ok()
    };
    let mut in_string = false;
    let mut at = 0;
    while at < bytes.len() {
        match (in_string, bytes[at]) {
            (_, b'"') => {
                in_string = !in_string;
                at += 1;
            }
            (true, b'\\') => match unit_at(at) {
                Some(high @ 0xD800..=0xDBFF) => match unit_at(at + 6) {
                    Some(low @ 0xDC00..=0xDFFF) => {
                        found(at..at + 6, high, false);
                        found(at + 6..at + 12, low, false);
                        at += 12;
                    }
                    _ => {
                        found(at..at + 6, high, true);
                        at += 6;
                    }
                },
                Some(unit) => {
                    found(at..at + 6, unit, (0xDC00..=0xDFFF).contains(&unit));
                    at += 6;
                }
                None => at += 2,
            },
            _ => at += 1,
        }
    }
}

/// The stand-ins of one text's lone surrogates: a private-use character for each code unit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Surrogates {
    by_unit: BTreeMap<u16, char>,
}

impl Surrogates {
    /// `text` with each lone surrogate escape spelled as its stand-in's, and the stand-ins: one for
    /// each code unit there or in `units`, none a character that `text` holds (as itself or
    /// escaped) or that `avoid` holds. `None` when the private-use area has too few left.
    pub fn stand_in(text: &str, units: &[u16], avoid: &str) -> Option<(String, Surrogates)> {
        let mut held: BTreeSet<char> = text.chars().chain(avoid.chars()).collect();
        let mut wanted: BTreeSet<u16> = units.iter().copied().collect();
        let mut lone = Vec::new();
        for_each_escape(text, |at, unit, is_lone| {
            if is_lone {
                wanted.insert(unit);
                lone.push((at, unit));
            } else if let Some(c) = char::from_u32(u32::from(unit)) {
                held.insert(c);
            }
        });
        let mut free = ('\u{E000}'..='\u{F8FF}').filter(|c| !held.contains(c));
        let mut by_unit = BTreeMap::new();
        for unit in wanted {
            by_unit.insert(unit, free.next()?);
        }
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for (at, unit) in lone {
            out.push_str(&text[last..at.start]);
            let stand_in = by_unit.get(&unit).copied().unwrap_or('\u{FFFD}');
            out.push_str(&format!("\\u{:04x}", u32::from(stand_in)));
            last = at.end;
        }
        out.push_str(&text[last..]);
        Some((out, Surrogates { by_unit }))
    }

    /// The stand-in for a surrogate code unit, if there is one.
    pub fn char_for(&self, unit: u16) -> Option<char> {
        self.by_unit.get(&unit).copied()
    }

    /// Whether `c` stands in for a surrogate.
    pub fn stands_in(&self, c: char) -> bool {
        self.by_unit.values().any(|stand_in| *stand_in == c)
    }

    /// JSON written ASCII-escaped by [`super::write()`] with each stand-in's escape spelled as its
    /// surrogate's again, as Python's `json.dumps` spells one.
    pub fn restore(&self, json: &str) -> String {
        let by_char: BTreeMap<u16, u16> = self
            .by_unit
            .iter()
            .filter_map(|(unit, c)| Some((u16::try_from(u32::from(*c)).ok()?, *unit)))
            .collect();
        let mut out = String::with_capacity(json.len());
        let mut last = 0;
        for_each_escape(json, |at, unit, _| {
            if let Some(surrogate) = by_char.get(&unit) {
                out.push_str(&json[last..at.start]);
                out.push_str(&format!("\\u{surrogate:04x}"));
                last = at.end;
            }
        });
        out.push_str(&json[last..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Format, Value, parse, write};

    #[test]
    fn lone_surrogates_are_found_and_pairs_are_not() {
        let text = r#"{"a\udcff": "x\ud83d\ude00y\ud800", "\\udcff": "\ude00", "k": 1}"#;
        let found: Vec<(&str, u16)> = lone_surrogate_escapes(text)
            .into_iter()
            .map(|(at, unit)| (&text[at], unit))
            .collect();
        assert_eq!(
            found,
            [
                (r"\udcff", 0xDCFF),
                (r"\ud800", 0xD800),
                (r"\ude00", 0xDE00)
            ]
        );
        assert_eq!(lone_surrogate_escapes(r#""\udcff"#).len(), 1);
        assert!(lone_surrogate_escapes(r#"["\u12"]"#).is_empty());
    }

    #[test]
    fn surrogates_survive_a_read_and_a_write() {
        // the text holds U+E000 itself and escaped U+E001: neither may stand in
        let text = r#"{"a\udcff": "\ue001\udcff", "b": "\u00e9\ud83d\ude00", "c": "\\udcfe"}"#;
        let (stood, surrogates) = Surrogates::stand_in(text, &[0xDC80], "\u{E000}").unwrap();
        assert_eq!(surrogates.char_for(0xDC80), Some('\u{E002}'));
        assert_eq!(surrogates.char_for(0xDCFF), Some('\u{E003}'));
        let value = parse(stood.as_bytes()).unwrap().value;
        assert!(!parse(stood.as_bytes()).unwrap().lone_surrogates);
        let Value::Object(mut members) = value else {
            panic!("an object");
        };
        members.insert("\u{E002}".into(), Value::from("new"));
        let written = write(&Value::Object(members), Format::INDENTED).unwrap();
        assert_eq!(
            surrogates.restore(&written),
            "{\n \"a\\udcff\": \"\\ue001\\udcff\",\n \"b\": \"\\u00e9\\ud83d\\ude00\",\n \
             \"c\": \"\\\\udcfe\",\n \"\\udc80\": \"new\"\n}"
        );
        assert!(surrogates.stands_in('\u{E003}') && !surrogates.stands_in('\u{E001}'));
    }
}
