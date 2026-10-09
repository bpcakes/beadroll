//! Python's text rules where git's output is read: `bytes.strip()`, `str.strip()` and
//! `str.splitlines()`, and arguments made from paths.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

/// `data.strip()` on bytes: without the ASCII whitespace at either end.
pub(crate) fn strip_ascii(data: &[u8]) -> &[u8] {
    let space = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C);
    let start = data.iter().position(|b| !space(b)).unwrap_or(data.len());
    let end = data
        .iter()
        .rposition(|b| !space(b))
        .map_or(start, |at| at + 1);
    data.get(start..end).unwrap_or_default()
}

/// `text.strip()`: without the whitespace Python's `str` sees at either end (Unicode's, and the
/// separators U+001C to U+001F).
pub(crate) fn py_strip(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `text.splitlines()`: lines split at every line boundary Python's `str` knows (`\n`, `\r`,
/// `\r\n`, `\x0b`, `\x0c`, `\x1c`-`\x1e`, `\x85`, U+2028, U+2029), without them.
pub(crate) fn py_splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let breaks = matches!(
            c,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !breaks {
            continue;
        }
        lines.push(&text[start..at]);
        start = at + c.len_utf8();
        if c == '\r' && chars.peek().is_some_and(|(_, next)| *next == '\n') {
            chars.next();
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// An argument for git from a path, byte for byte.
pub fn path_arg(path: &Path) -> OsString {
    OsStr::from_bytes(path.as_os_str().as_bytes()).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_split_as_python_splits_them() {
        assert_eq!(
            py_splitlines("a\r\nb\rc\n\nd\u{2028}e\n"),
            ["a", "b", "c", "", "d", "e"]
        );
        assert_eq!(py_splitlines(""), Vec::<&str>::new());
        assert_eq!(py_splitlines("x"), ["x"]);
    }

    #[test]
    fn strips_follow_python() {
        assert_eq!(strip_ascii(b"\x0b\x0c x\t\n"), b"x");
        assert_eq!(py_strip("\u{1c}\u{a0} x \u{3000}"), "x");
    }
}
