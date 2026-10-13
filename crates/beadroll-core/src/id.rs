//! Event ids: fixed-width lowercase UUIDv7 strings (`8-4-4-4-12` hex), held as numbers. Numeric
//! and lexical ordering agree.

use std::fmt;

/// An event id: the UUID shape exactly, with no trailing newline (U02 in `docs/FINDINGS.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(u128);

/// The groups of a UUID's hex digits, dash-separated.
const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

impl EventId {
    /// The event id spelled by `text`, if it matches the UUID shape exactly.
    pub fn parse(text: &str) -> Option<EventId> {
        let bytes = text.as_bytes();
        if bytes.len() != 36 {
            return None;
        }
        let mut value = 0u128;
        let mut at = 0;
        for (index, width) in GROUPS.into_iter().enumerate() {
            if index > 0 {
                if bytes.get(at) != Some(&b'-') {
                    return None;
                }
                at += 1;
            }
            for _ in 0..width {
                let digit = match bytes.get(at)? {
                    byte @ b'0'..=b'9' => byte - b'0',
                    byte @ b'a'..=b'f' => byte - b'a' + 10,
                    _ => return None,
                };
                value = (value << 4) | u128::from(digit);
                at += 1;
            }
        }
        Some(EventId(value))
    }

    /// The id whose 128 bits are `value`.
    pub fn from_u128(value: u128) -> EventId {
        EventId(value)
    }

    /// Its 128 bits.
    pub fn as_u128(self) -> u128 {
        self.0
    }

    /// The timestamp in milliseconds, from the first 48 bits.
    pub fn millis(self) -> u64 {
        (self.0 >> 80) as u64
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex = format!("{:032x}", self.0);
        let mut at = 0;
        for (index, width) in GROUPS.into_iter().enumerate() {
            if index > 0 {
                f.write_str("-")?;
            }
            f.write_str(hex.get(at..at + width).unwrap_or_default())?;
            at += width;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_the_uuid_shape_exactly() {
        let text = "01a11621-e285-7398-ab1c-b9af4864eaa2";
        let id = EventId::parse(text).unwrap();
        assert_eq!(id.to_string(), text);
        assert_eq!(id.millis(), 0x01a1_1621_e285);
        for bad in [
            "01a11621-e285-7398-ab1c-b9af4864eaa2\n",
            "01A11621-e285-7398-ab1c-b9af4864eaa2",
            "01a11621e2857398ab1cb9af4864eaa2",
            "01a11621-e285-7398-ab1c-b9af4864eaa",
            "01a11621-e285-7398-ab1c_b9af4864eaa2",
            "",
        ] {
            assert!(EventId::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn ids_order_as_their_strings() {
        let mut texts = [
            "019a5555-5555-7000-8000-00000000000a",
            "019a5555-5555-7000-8000-000000000009",
            "ffffffff-ffff-7fff-bfff-ffffffffffff",
            "00000000-0000-7000-8000-000000000000",
        ];
        let mut ids: Vec<EventId> = texts.iter().filter_map(|t| EventId::parse(t)).collect();
        texts.sort_unstable();
        ids.sort_unstable();
        let shown: Vec<String> = ids.iter().map(EventId::to_string).collect();
        assert_eq!(shown, texts);
    }
}
