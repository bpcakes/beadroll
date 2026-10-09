//! Issue keys: the number a key ends in, and the keys bead makes when it mints one or renames a copy
//! two machines both minted (`free_key`, `mint_key`).

use sha2::{Digest, Sha256};

use crate::json::{Int, MAX_DIGITS};
use crate::text::b36;

/// A key that ends in `.` and a number, as `(.+)\.([0-9]+)` matches it whole: what comes before the
/// last dot (no newline in it), and the ASCII digits after it (`docs/RUST-PORT.md` §5 row 24).
pub fn split_number(key: &str) -> Option<(&str, &str)> {
    let (stem, digits) = key.rsplit_once('.')?;
    let numbered = !stem.is_empty()
        && !stem.contains('\n')
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit());
    numbered.then_some((stem, digits))
}

/// Whether a key's number is too long for a renamed copy to count up from: Python prints at most
/// [`MAX_DIGITS`] digits, and counting up from that many nines needs one more. A create naming
/// such a key is refused (`docs/RUST-PORT.md` §5 row 25).
pub fn number_too_long(key: &str) -> bool {
    split_number(key).is_some_and(|(_, digits)| digits.len() >= MAX_DIGITS)
}

/// The key a copy keeps when `key` is taken (Python's `State.free_key`): a numbered key counts on
/// from its number to the first free one; any other gets the shortest prefix of its create's
/// base-36 digest that makes it free. `None` if every prefix is taken.
pub fn free_key(key: &str, uid: &str, taken: &dyn Fn(&str) -> bool) -> Option<String> {
    if let Some((stem, digits)) = split_number(key) {
        let mut number = Int::parse(digits)?;
        loop {
            let candidate = format!("{stem}.{number}");
            if !taken(&candidate) {
                return Some(candidate);
            }
            number = number.plus_one();
        }
    }
    let digest = b36(&Sha256::digest(uid.as_bytes()));
    (1..digest.len()).find_map(|size| {
        let candidate = format!("{key}{}", digest.get(..size)?);
        (!taken(&candidate)).then_some(candidate)
    })
}

/// Why a key cannot be minted (Python's `mint_key` raises).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MintError {
    /// Every digest prefix is taken: `KEY_SPACE`.
    #[error("cannot mint a key")]
    KeySpace,
    /// A sibling's number is too long to read: `VALIDATION` where Python raises
    /// (`docs/RUST-PORT.md` §5 row 26).
    #[error("a key under {0} ends in a number of more than {MAX_DIGITS} digits")]
    NumberTooLong(String),
}

/// A key no issue has and no imported dependency waits for (Python's `mint_key`): under a parent,
/// the parent's key and one more than the highest number among its children's keys; otherwise the
/// prefix and the shortest prefix (four characters at least) of the seed's base-36 digest that is
/// free.
pub fn mint_key(
    prefix: &str,
    seed: &str,
    parent: Option<&str>,
    taken: &[&str],
) -> Result<String, MintError> {
    if let Some(parent) = parent {
        let mut highest: Option<Int> = None;
        for key in taken {
            // `re.escape(parent) + r"\.([0-9]+)"`, matched whole: the parent's key exactly, newlines
            // and all, then a dot and ASCII digits.
            let Some(digits) = key
                .strip_prefix(parent)
                .and_then(|rest| rest.strip_prefix('.'))
            else {
                continue;
            };
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let number =
                Int::parse(digits).ok_or_else(|| MintError::NumberTooLong(parent.to_owned()))?;
            if highest.as_ref().is_none_or(|h| number > *h) {
                highest = Some(number);
            }
        }
        let next = highest.unwrap_or_else(|| Int::from(0)).plus_one();
        if next.digit_count() > MAX_DIGITS {
            return Err(MintError::NumberTooLong(parent.to_owned()));
        }
        return Ok(format!("{parent}.{next}"));
    }
    let digest = b36(&Sha256::digest(seed.as_bytes()));
    (4..digest.len())
        .find_map(|size| {
            let candidate = format!("{prefix}-{}", digest.get(..size)?);
            (!taken.contains(&candidate.as_str())).then_some(candidate)
        })
        .ok_or(MintError::KeySpace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_number_is_ascii_digits_after_the_last_dot() {
        assert_eq!(split_number("ex-1.2"), Some(("ex-1", "2")));
        assert_eq!(split_number("ex-a.1.20"), Some(("ex-a.1", "20")));
        for none in ["ex-1", "ex.", ".5", "a\nb.5", "x.\u{663}", "x.1a", "x.-1"] {
            assert_eq!(split_number(none), None, "{none:?}");
        }
        assert!(number_too_long(&format!("x.{}", "1".repeat(MAX_DIGITS))));
        assert!(!number_too_long(&format!(
            "x.{}",
            "9".repeat(MAX_DIGITS - 1)
        )));
    }

    #[test]
    fn free_key_counts_on_or_extends_the_key() {
        let taken = |k: &str| ["x.7", "x.8", "ex-y", "ex-y4"].contains(&k);
        assert_eq!(free_key("x.007", "u", &taken).as_deref(), Some("x.9"));
        let renamed = free_key("ex-y", "01a11621-e285-7398-ab1c-b9af4864eaa2", &taken).unwrap();
        assert!(
            renamed.starts_with("ex-y") && renamed.len() > 4,
            "{renamed}"
        );
    }
}
