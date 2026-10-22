//! Issue keys: the number a key ends in, and the keys bead makes when it mints one or renames a copy
//! two machines both minted (`free_key`, `mint_key`).

use sha2::{Digest, Sha256};

use crate::json::Int;
use crate::text::b36;

/// A key that ends in `.` and a number, as `(.+)\.([0-9]+)` matches it whole: what comes before the
/// last dot (no newline in it), and the ASCII digits after it (U06 in `docs/FINDINGS.md`).
pub fn split_number(key: &str) -> Option<(&str, &str)> {
    let (stem, digits) = key.rsplit_once('.')?;
    let numbered = !stem.is_empty()
        && !stem.contains('\n')
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit());
    numbered.then_some((stem, digits))
}

/// Whether a key's number leaves no room to count on: it is beyond what bead counts (an `i128`),
/// or one below it. A key with such a number is refused at creation so a duplicate can still be
/// renamed (U07 in `docs/FINDINGS.md`).
pub fn number_too_long(key: &str) -> bool {
    split_number(key).is_some_and(|(_, digits)| {
        Int::parse(digits)
            .and_then(|number| number.plus_one())
            .is_none()
    })
}

/// A free key for a duplicate: numbered keys count upward; others take the shortest free prefix
/// of the create's base-36 digest. Returns `None` if every prefix is taken, or the numbers run out.
pub fn free_key(key: &str, uid: &str, taken: &dyn Fn(&str) -> bool) -> Option<String> {
    if let Some((stem, digits)) = split_number(key) {
        let mut number = Int::parse(digits)?;
        loop {
            let candidate = format!("{stem}.{number}");
            if !taken(&candidate) {
                return Some(candidate);
            }
            number = number.plus_one()?;
        }
    }
    let digest = b36(&Sha256::digest(uid.as_bytes()));
    (1..digest.len()).find_map(|size| {
        let candidate = format!("{key}{}", digest.get(..size)?);
        (!taken(&candidate)).then_some(candidate)
    })
}

/// Why a key cannot be minted.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MintError {
    /// Every digest prefix is taken: `KEY_SPACE`.
    #[error("cannot mint a key")]
    KeySpace,
    /// A sibling's number is beyond what bead counts, or the next would be: VALIDATION.
    #[error("a key under {0} ends in a number beyond what bead counts")]
    NumberTooLong(String),
}

/// A key held by neither an issue nor an imported dependency. Under a parent, use one more than
/// its highest numbered child. Otherwise use the prefix and shortest free digest prefix of at
/// least four characters.
pub fn mint_key(
    prefix: &str,
    seed: &str,
    parent: Option<&str>,
    taken: &[&str],
) -> Result<String, MintError> {
    if let Some(parent) = parent {
        let too_long = || MintError::NumberTooLong(parent.to_owned());
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
            let number = Int::parse(digits).ok_or_else(too_long)?;
            if highest.as_ref().is_none_or(|h| number > *h) {
                highest = Some(number);
            }
        }
        let next = highest
            .unwrap_or_else(|| Int::from(0))
            .plus_one()
            .ok_or_else(too_long)?;
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

    /// The largest number bead counts, and the one below it.
    const MOST: &str = "170141183460469231731687303715884105727";
    const BELOW: &str = "170141183460469231731687303715884105726";

    #[test]
    fn a_key_number_is_ascii_digits_after_the_last_dot() {
        assert_eq!(split_number("ex-1.2"), Some(("ex-1", "2")));
        assert_eq!(split_number("ex-a.1.20"), Some(("ex-a.1", "20")));
        for none in ["ex-1", "ex.", ".5", "a\nb.5", "x.\u{663}", "x.1a", "x.-1"] {
            assert_eq!(split_number(none), None, "{none:?}");
        }
        assert!(number_too_long(&format!("x.{MOST}")));
        assert!(number_too_long(&format!("x.{}", "1".repeat(40))));
        assert!(!number_too_long(&format!("x.{BELOW}")));
        assert!(!number_too_long("x.7"));
        assert!(!number_too_long("ex-1"));
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
        // The numbers run out where bead stops counting, instead of looping.
        let most = format!("x.{MOST}");
        assert_eq!(free_key(&most, "u", &|k| k == most), None);
        assert_eq!(
            free_key(&format!("x.{BELOW}"), "u", &taken).as_deref(),
            Some(&*format!("x.{BELOW}"))
        );
    }

    #[test]
    fn minting_under_a_parent_counts_on_from_the_highest_child() {
        assert_eq!(
            mint_key("ex", "s", Some("ex-1"), &["ex-1.2", "ex-1.10", "ex-2.99"]).as_deref(),
            Ok("ex-1.11")
        );
        assert_eq!(
            mint_key("ex", "s", Some("ex-1"), &[]).as_deref(),
            Ok("ex-1.1")
        );
        let most = format!("ex-1.{MOST}");
        assert_eq!(
            mint_key("ex", "s", Some("ex-1"), &[&most]),
            Err(MintError::NumberTooLong("ex-1".into()))
        );
        let beyond = format!("ex-1.{}", "1".repeat(40));
        assert_eq!(
            mint_key("ex", "s", Some("ex-1"), &[&beyond]),
            Err(MintError::NumberTooLong("ex-1".into()))
        );
        assert_eq!(
            mint_key("ex", "s", Some("ex-1"), &[&format!("ex-1.{BELOW}")]).as_deref(),
            Ok(&*format!("ex-1.{MOST}"))
        );
    }
}
