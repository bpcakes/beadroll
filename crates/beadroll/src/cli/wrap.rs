//! Wrapping help text: split into chunks of spaces, words, hyphenated word parts and em-dashes,
//! then fit as many chunks as possible on each line. Drop spaces at line ends and break an
//! overlong word after its last hyphen when possible.

use beadroll_core::text::{decimal_value, strip};

/// What `HelpFormatter` wraps: every run of ASCII whitespace one space, and no whitespace at
/// either end.
pub(crate) fn collapsed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if c.is_ascii_whitespace() || c == '\u{0b}' {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    strip(&out).to_owned()
}

/// `textwrap.wrap(text, width, initial_indent=indent, subsequent_indent=indent)` for a text
/// [`collapsed`] made: lines no longer than `width`, the indent included.
pub(crate) fn wrap(text: &str, width: i64, indent: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut chunks = split(&chars);
    chunks.reverse();
    let width = width - length(indent.chars().count());
    let mut lines: Vec<String> = Vec::new();
    while !chunks.is_empty() {
        let mut line: Vec<Vec<char>> = Vec::new();
        let mut used = 0;
        // a line starts with no space, except the first
        if !lines.is_empty() && chunks.last().is_some_and(|c| blank(c)) {
            chunks.pop();
        }
        while let Some(chunk) = chunks.last() {
            if used + length(chunk.len()) > width {
                break;
            }
            used += length(chunk.len());
            line.extend(chunks.pop());
        }
        if chunks.last().is_some_and(|c| length(c.len()) > width) {
            long_word(&mut chunks, &mut line, used, width);
        }
        if line.last().is_some_and(|c| blank(c)) {
            line.pop();
        }
        if !line.is_empty() {
            let text: String = line.concat().into_iter().collect();
            lines.push(format!("{indent}{text}"));
        }
    }
    lines
}

fn length(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// Whether a chunk is whitespace (`chunk.strip() == ''`).
fn blank(chunk: &[char]) -> bool {
    let text: String = chunk.iter().collect();
    strip(&text).is_empty()
}

/// `_handle_long_word`: as much of a chunk too long for any line as fits on this one, up to and
/// including its last hyphen when there is one with something other than hyphens before it.
fn long_word(chunks: &mut [Vec<char>], line: &mut Vec<Vec<char>>, used: i64, width: i64) {
    let space_left = if width < 1 { 1 } else { width - used };
    let Some(chunk) = chunks.last_mut() else {
        return;
    };
    if space_left > 0 {
        let mut end = usize::try_from(space_left).unwrap_or(0).min(chunk.len());
        if chunk.len() > end
            && let Some(hyphen) = chunk[..end].iter().rposition(|c| *c == '-')
            && hyphen > 0
            && chunk[..hyphen].iter().any(|c| *c != '-')
        {
            end = hyphen + 1;
        }
        line.push(chunk[..end].to_vec());
        chunk.drain(..end);
    } else if line.is_empty() {
        line.push(std::mem::take(chunk));
    }
}

/// textwrap's whitespace: ASCII's.
fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r' | ' ')
}

/// Word characters: alphanumerics and `_`. Help text is ASCII.
fn is_word(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// `[^\d\W]`: a word character that is not a digit.
fn is_letter(c: char) -> bool {
    is_word(c) && decimal_value(c).is_none()
}

/// `[\w!"'&.,?]`: what may end a word before an em-dash.
fn is_word_punct(c: char) -> bool {
    is_word(c) || "!\"'&.,?".contains(c)
}

/// The text cut as `TextWrapper.wordsep_re` splits it.
fn split(text: &[char]) -> Vec<Vec<char>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let size = chunk_at(text, at).max(1);
        out.push(text[at..(at + size).min(text.len())].to_vec());
        at += size;
    }
    out
}

/// The length of the chunk at `p`: a run of whitespace; an em-dash (two hyphens or more after a
/// word and before one); or the shortest run of non-space ending at a hyphen inside a word, at
/// the end of the word, or before an em-dash.
fn chunk_at(t: &[char], p: usize) -> usize {
    let at = |i: usize| t.get(i).copied();
    let dashes = |from: usize| {
        t.get(from..)
            .unwrap_or_default()
            .iter()
            .take_while(|c| **c == '-')
            .count()
    };
    let em_dash_at = |from: usize| {
        let run = dashes(from);
        run >= 2 && at(from + run).is_some_and(is_word)
    };
    if at(p).is_some_and(is_space) {
        return t[p..].iter().take_while(|c| is_space(**c)).count();
    }
    if p > 0 && is_word_punct(t[p - 1]) && em_dash_at(p) {
        return dashes(p);
    }
    let mut k = 1;
    loop {
        let end = p + k;
        if at(end) == Some('-') && hyphen_breaks(t, end) {
            return k + 1;
        }
        if at(end).is_none_or(is_space) {
            return k;
        }
        if is_word_punct(t[end - 1]) && em_dash_at(end) {
            return k;
        }
        k += 1;
    }
}

/// Whether a word may break after the hyphen at `h`: two letters (or a letter, a hyphen and a
/// letter) before it, and a letter, perhaps a hyphen, and a letter after it.
fn hyphen_breaks(t: &[char], h: usize) -> bool {
    let letter = |i: Option<usize>| i.and_then(|i| t.get(i)).is_some_and(|c| is_letter(*c));
    let hyphen = |i: Option<usize>| i.and_then(|i| t.get(i)) == Some(&'-');
    let before = (letter(h.checked_sub(2)) && letter(h.checked_sub(1)))
        || (letter(h.checked_sub(3)) && hyphen(h.checked_sub(2)) && letter(h.checked_sub(1)));
    let after = letter(Some(h + 1))
        && (letter(Some(h + 2)) || (hyphen(Some(h + 2)) && letter(Some(h + 3))));
    before && after
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        split(&chars)
            .into_iter()
            .map(|c| c.into_iter().collect())
            .collect()
    }

    #[test]
    fn text_splits_as_textwrap_splits_it() {
        // textwrap's own example
        assert_eq!(
            chunks("Hello there -- you goof-ball, use the -b option!"),
            [
                "Hello", " ", "there", " ", "--", " ", "you", " ", "goof-", "ball,", " ", "use",
                " ", "the", " ", "-b", " ", "option!"
            ]
        );
        assert_eq!(
            chunks("A B [--type blocks|parent-child], dep"),
            [
                "A",
                " ",
                "B",
                " ",
                "[--type",
                " ",
                "blocks|parent-",
                "child],",
                " ",
                "dep"
            ]
        );
        assert_eq!(chunks("well--said"), ["well", "--", "said"]);
    }

    #[test]
    fn lines_fill_the_width_and_long_words_break() {
        assert_eq!(
            wrap("only issues in ID's subtree: its children", 20, ""),
            ["only issues in ID's", "subtree: its", "children"]
        );
        assert_eq!(
            wrap("comma list of [type:]ISSUE", 11, ""),
            ["comma list", "of [type:]I", "SSUE"]
        );
        assert_eq!(wrap("abc-defghijklmnop", 11, ""), ["abc-defghij", "klmnop"]);
        assert_eq!(wrap("a b", 70, "  "), ["  a b"]);
        assert_eq!(collapsed("  a \t b\n"), "a b");
    }
}
