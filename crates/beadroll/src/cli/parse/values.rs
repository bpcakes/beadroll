//! What argparse makes of an argument's strings: its default, its value converted to its type,
//! how many strings an option or a positional takes (`_match_argument`, `_get_nargs_pattern`),
//! and whether a string is a negative number, which is a value.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::text::{decimal_value, py_int, repr_bytes, repr_str};

use super::super::spec::{Action, ArgSpec, COMMANDS, Kind, Nargs, find};
use super::{Stop, Value, argument_error};

/// An action's value before anything is given: its default, `False` for a flag.
pub(super) fn default(action: &ArgSpec) -> Value {
    match (action.action, action.kind, action.default) {
        (Action::StoreTrue, _, _) => Value::Bool(false),
        (_, Kind::Int, Some(number)) => py_int(number).map_or(Value::None, Value::Int),
        (_, _, Some(text)) => Value::Text(OsString::from(text)),
        _ => Value::None,
    }
}

/// The value an action is given for `strings` (`_get_values`): converted to its type, and a
/// command checked against the commands.
pub(super) fn values_of(action: &ArgSpec, strings: &[OsString]) -> Result<Value, Stop> {
    match (action.nargs, strings) {
        (Nargs::Optional, []) => Ok(default(action)),
        (Nargs::One | Nargs::Optional, [one]) => converted(action, one),
        (Nargs::Parser, [name, ..]) => {
            if name.to_str().and_then(find).is_none() {
                let choices: Vec<String> = COMMANDS.iter().map(|c| repr_str(c.name)).collect();
                let message = format!(
                    "invalid choice: {} (choose from {})",
                    repr_bytes(name.as_bytes()),
                    choices.join(", ")
                );
                return Err(argument_error(action, message));
            }
            Ok(Value::List(strings.to_vec()))
        }
        _ => Ok(Value::List(strings.to_vec())),
    }
}

/// One value converted to the action's type (`_get_value`): `int()` for a number.
pub(super) fn converted(action: &ArgSpec, string: &OsStr) -> Result<Value, Stop> {
    match action.kind {
        Kind::Text => Ok(Value::Text(string.to_owned())),
        Kind::Int => string
            .to_str()
            .and_then(py_int)
            .map(Value::Int)
            .ok_or_else(|| {
                let message = format!("invalid int value: {}", repr_bytes(string.as_bytes()));
                argument_error(action, message)
            }),
    }
}

/// How many of the arguments that follow an option it takes (`_match_argument`), or argparse's
/// complaint. bead's options take one value or none.
pub(super) fn option_count(action: &ArgSpec, pattern: &[u8]) -> Result<usize, Stop> {
    let a_run = pattern.iter().take_while(|p| **p == b'A').count();
    let count = match action.nargs {
        Nargs::Zero => Some(0),
        Nargs::One => (a_run >= 1).then_some(1),
        Nargs::Optional => Some(a_run.min(1)),
        Nargs::ZeroOrMore => Some(a_run),
        Nargs::OneOrMore => (a_run >= 1).then_some(a_run),
        Nargs::Parser => (a_run >= 1).then_some(pattern.len()),
    };
    count.ok_or_else(|| {
        argument_error(
            action,
            match action.nargs {
                Nargs::Optional => "expected at most one argument",
                Nargs::OneOrMore => "expected at least one argument",
                _ => "expected one argument",
            },
        )
    })
}

/// How many arguments from the start of `pattern` a positional's `nargs` pattern matches
/// greedily (`-*A-*`, `-*A?-*`, `-*[A-]*`, `-*A[A-]*`, `-*A[-AO]*`), or `None`.
pub(super) fn positional_count(nargs: Nargs, pattern: &[u8]) -> Option<usize> {
    let run = |from: usize, of: &[u8]| {
        pattern
            .get(from..)
            .unwrap_or_default()
            .iter()
            .take_while(|p| of.contains(p))
            .count()
    };
    let dashes = run(0, b"-");
    let at_a = pattern.get(dashes) == Some(&b'A');
    match nargs {
        Nargs::One => at_a.then(|| dashes + 1 + run(dashes + 1, b"-")),
        Nargs::Optional => {
            let a = usize::from(at_a);
            Some(dashes + a + run(dashes + a, b"-"))
        }
        Nargs::ZeroOrMore => Some(run(0, b"A-")),
        Nargs::OneOrMore => at_a.then(|| dashes + 1 + run(dashes + 1, b"A-")),
        Nargs::Parser => at_a.then_some(pattern.len()),
        Nargs::Zero => Some(0),
    }
}

/// argparse's `-\.?\d` at the start: a negative number, which is a value.
pub(super) fn looks_negative(arg: &[u8]) -> bool {
    let rest = arg.strip_prefix(b"-").unwrap_or_default();
    let rest = rest.strip_prefix(b".").unwrap_or(rest);
    let head = &rest[..rest.len().min(4)];
    String::from_utf8_lossy(head)
        .chars()
        .next()
        .is_some_and(|c| decimal_value(c).is_some())
}
