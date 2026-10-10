//! Whether an argument is an option, and which (`_parse_optional`, `_get_option_tuples`): an
//! option string, an unambiguous prefix of a long one, a short one with its value attached; a
//! negative number, or anything with a space in it, is a value.

use super::Parser;
use super::values::looks_negative;

/// An option an argument may be (argparse's option tuple): the action, unless no option of the
/// parser matched, the option string it matched, and the value given with it (`--title=x`,
/// `-tx`), `eq` when an `=` separated it.
#[derive(Clone, Debug)]
pub(super) struct Candidate {
    pub(super) action: Option<usize>,
    pub(super) option: &'static str,
    pub(super) eq: bool,
    pub(super) explicit: Option<Vec<u8>>,
}

/// The options `arg` may be, or `None` when it is a value (`_parse_optional`).
pub(super) fn parse_optional(parser: &Parser, arg: &[u8]) -> Option<Vec<Candidate>> {
    if arg.first() != Some(&b'-') {
        return None;
    }
    if let Some((option, index)) = parser.option(arg) {
        return Some(vec![Candidate {
            action: Some(index),
            option,
            eq: false,
            explicit: None,
        }]);
    }
    if arg.len() == 1 {
        return None;
    }
    if let Some(at) = arg.iter().position(|b| *b == b'=')
        && let Some((option, index)) = parser.option(&arg[..at])
    {
        return Some(vec![Candidate {
            action: Some(index),
            option,
            eq: true,
            explicit: Some(arg[at + 1..].to_vec()),
        }]);
    }
    let candidates = option_tuples(parser, arg);
    if !candidates.is_empty() {
        return Some(candidates);
    }
    // no option of bead's looks like a negative number, so one is a value
    if looks_negative(arg) || arg.contains(&b' ') {
        return None;
    }
    Some(vec![Candidate {
        action: None,
        option: "",
        eq: false,
        explicit: None,
    }])
}

/// The options `arg` is a prefix of, or (`-tx`) a short option with its value attached
/// (`_get_option_tuples`). bead's option strings are ASCII, so bytes compare as characters do.
fn option_tuples(parser: &Parser, arg: &[u8]) -> Vec<Candidate> {
    let (prefix, eq, explicit) = match arg.iter().position(|b| *b == b'=') {
        Some(at) => (&arg[..at], true, Some(arg[at + 1..].to_vec())),
        None => (arg, false, None),
    };
    let long = arg.get(1) == Some(&b'-');
    let mut out = Vec::new();
    for (option, index) in parser.options() {
        if !long && option.as_bytes() == &arg[..2] {
            out.push(Candidate {
                action: Some(index),
                option,
                eq: false,
                explicit: Some(arg[2..].to_vec()),
            });
        } else if option.as_bytes().starts_with(prefix) {
            out.push(Candidate {
                action: Some(index),
                option,
                eq,
                explicit: explicit.clone(),
            });
        }
    }
    out
}
