//! Command-line text with validity tracked separately. Bytes that are not UTF-8 match no key,
//! status or name, print as `?`, and fail only when an event carrying them is encoded. They are
//! represented here by U+FFFD and a flag, preserving that validation order.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::code::Code;
use beadroll_core::event::STATUSES;
use beadroll_core::json::{Int, Object, Value};
use beadroll_core::state::{Issue, State};
use beadroll_core::text::{is_space, repr_bytes, strip};

use super::{Failure, issue_not_found, parse_priority, priority_out_of_range, read_utf8, resolve};

/// Whether command-line text can be encoded in an event. It travels with the text from where the
/// argument is read to the [`encodable`] check after the event is emitted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TextValidity {
    /// Some bytes are not UTF-8: they are held as U+FFFD, and writing the text fails.
    Invalid,
    /// Every byte is UTF-8 (no text at all is too): the text can be written.
    #[default]
    Utf8,
}

impl TextValidity {
    /// The validity of this text and `other` written together: valid only if both are.
    pub(crate) fn and(self, other: TextValidity) -> TextValidity {
        match (self, other) {
            (TextValidity::Utf8, TextValidity::Utf8) => TextValidity::Utf8,
            _ => TextValidity::Invalid,
        }
    }
}

/// Command-line text and its UTF-8 validity. Invalid bytes are represented by U+FFFD; they match
/// no key, status or name and cause a write to fail when encoded.
pub(crate) fn arg_text(value: &OsStr) -> (String, TextValidity) {
    match value.to_str() {
        Some(text) => (text.to_owned(), TextValidity::Utf8),
        None => (value.to_string_lossy().into_owned(), TextValidity::Invalid),
    }
}

/// [`arg_text`] of each, and the validity of them all.
pub(crate) fn arg_texts(values: &[OsString]) -> (Vec<String>, TextValidity) {
    let mut valid = TextValidity::Utf8;
    let texts = values
        .iter()
        .map(|value| {
            let (text, ok) = arg_text(value);
            valid = valid.and(ok);
            text
        })
        .collect();
    (texts, valid)
}

/// An event payload and whether it contains command-line text that is not valid UTF-8. Encoding
/// checks validity after all other validation ([`encodable`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct Payload {
    pub(crate) values: Object,
    pub(crate) valid: TextValidity,
}

impl Payload {
    pub(crate) fn new() -> Payload {
        Payload::default()
    }

    /// Sets `name` to command-line text, in place when it is there already.
    pub(crate) fn set_text(&mut self, name: &str, value: &OsStr) {
        let (text, valid) = arg_text(value);
        self.valid = self.valid.and(valid);
        self.values.insert(name.into(), Value::from(text));
    }
}

/// Checks that an emitted event can be encoded: command-line text that is not valid UTF-8 fails
/// after all other validation. A failed transaction writes nothing.
pub(crate) fn encodable(valid: TextValidity) -> Result<(), Failure> {
    match valid {
        TextValidity::Utf8 => Ok(()),
        TextValidity::Invalid => Err(Failure::new(Code::Validation, "text is not valid UTF-8")),
    }
}

/// Comma-separated command-line items, trimmed, with empty items dropped and bytes that are not
/// UTF-8 preserved.
pub(crate) fn split_args(values: &[OsString]) -> Vec<OsString> {
    values
        .iter()
        .flat_map(|value| value.as_bytes().split(|b| *b == b','))
        .map(strip_arg)
        .filter(|item| !item.is_empty())
        .map(|item| OsStr::from_bytes(item).to_os_string())
        .collect()
}

/// Command-line bytes without Unicode whitespace or separators U+001C to U+001F at either end.
/// Bytes that are not UTF-8 are not whitespace.
fn strip_arg(data: &[u8]) -> &[u8] {
    let mut units: Vec<(usize, usize, Option<char>)> = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let (valid, bad) = match std::str::from_utf8(&data[at..]) {
            Ok(_) => (data.len() - at, 0),
            Err(error) => (
                error.valid_up_to(),
                error
                    .error_len()
                    .unwrap_or(data.len() - at - error.valid_up_to()),
            ),
        };
        let text = std::str::from_utf8(&data[at..at + valid]).unwrap_or_default();
        for (offset, c) in text.char_indices() {
            units.push((at + offset, at + offset + c.len_utf8(), Some(c)));
        }
        for byte in at + valid..at + valid + bad {
            units.push((byte, byte + 1, None));
        }
        at += valid + bad;
    }
    let space = |unit: &&(usize, usize, Option<char>)| unit.2.is_some_and(is_space);
    let start = units.iter().find(|u| !space(u)).map_or(data.len(), |u| u.0);
    let end = units
        .iter()
        .rev()
        .find(|u| !space(u))
        .map_or(start, |u| u.1);
    &data[start..end.max(start)]
}

/// `spec.rpartition(sep)` of command-line bytes: before the last `sep` and after it, or nothing
/// and all of it.
pub(crate) fn rpartition(spec: &OsStr, sep: u8) -> (&OsStr, &OsStr) {
    let bytes = spec.as_bytes();
    match bytes.iter().rposition(|b| *b == sep) {
        Some(at) => (
            OsStr::from_bytes(&bytes[..at]),
            OsStr::from_bytes(&bytes[at + 1..]),
        ),
        None => (OsStr::new(""), spec),
    }
}

/// The issue a command-line key names; a key that is not UTF-8 names none.
pub(crate) fn resolve_arg<'s>(st: &'s State, key: &OsStr) -> Result<&'s Issue, Failure> {
    match key.to_str() {
        Some(key) => resolve(st, key),
        None => Err(issue_not_found(key.as_bytes())),
    }
}

/// A priority supplied on the command line.
pub(crate) fn parse_priority_arg(value: &OsStr) -> Result<i64, Failure> {
    match value.to_str() {
        Some(value) => parse_priority(value),
        None => Err(priority_out_of_range(&repr_bytes(value.as_bytes()))),
    }
}

/// The content flags shared by `create` and `update`, as given.
#[derive(Default)]
pub(crate) struct ContentArgs {
    pub(crate) description: Option<OsString>,
    pub(crate) description_file: Option<OsString>,
    pub(crate) acceptance_criteria: Option<OsString>,
    pub(crate) notes: Option<OsString>,
    pub(crate) design: Option<OsString>,
    pub(crate) priority: Option<OsString>,
    pub(crate) issue_type: Option<OsString>,
    pub(crate) status: Option<OsString>,
    pub(crate) assignee: Option<OsString>,
    pub(crate) estimate: Option<Int>,
    pub(crate) external_ref: Option<OsString>,
    pub(crate) parent: Option<OsString>,
}

/// The supplied fields: texts first, then priority, type, estimate and status. An invalid
/// priority or status is refused here; text that is not UTF-8 is refused when the event is
/// written ([`Payload`]).
pub(crate) fn field_args(title: Option<&OsStr>, flags: &ContentArgs) -> Result<Payload, Failure> {
    let mut fields = Payload::new();
    let texts = [
        ("title", title),
        ("description", flags.description.as_deref()),
        ("acceptance_criteria", flags.acceptance_criteria.as_deref()),
        ("notes", flags.notes.as_deref()),
        ("design", flags.design.as_deref()),
        ("assignee", flags.assignee.as_deref()),
        ("external_ref", flags.external_ref.as_deref()),
    ];
    for (name, value) in texts {
        match (name, flags.description_file.as_deref()) {
            ("description", Some(path)) if !path.is_empty() => {
                let text = read_utf8(path, OsStr::new(name))?;
                fields.values.insert(name.into(), Value::from(text));
            }
            _ => {
                if let Some(value) = value {
                    fields.set_text(name, value);
                }
            }
        }
    }
    if let Some(priority) = &flags.priority {
        let priority = parse_priority_arg(priority)?;
        fields
            .values
            .insert("priority".into(), Value::from(priority));
    }
    // an empty type or parent is not one: refused, as an empty title is, never read as not given
    if flags
        .issue_type
        .as_deref()
        .is_some_and(|t| strip(&t.to_string_lossy()).is_empty())
    {
        return Err(Failure::new(
            Code::Validation,
            "--type needs a type, such as task or bug",
        ));
    }
    if flags
        .parent
        .as_deref()
        .is_some_and(|p| strip(&p.to_string_lossy()).is_empty())
    {
        return Err(Failure::new(
            Code::Validation,
            "--parent needs an issue's key; take an issue out of its parent with \
             `bead dep remove CHILD PARENT`",
        ));
    }
    if let Some(kind) = flags.issue_type.as_deref() {
        fields.set_text("issue_type", kind);
    }
    if let Some(estimate) = &flags.estimate {
        fields
            .values
            .insert("estimated_minutes".into(), Value::Int(estimate.clone()));
    }
    // an empty status is none: refused, as an empty type is, never read as not given
    if flags
        .status
        .as_deref()
        .is_some_and(|s| strip(&s.to_string_lossy()).is_empty())
    {
        return Err(Failure::new(
            Code::Validation,
            "--status needs a status, such as open or closed",
        ));
    }
    if let Some(status) = flags.status.as_deref() {
        let Some(status) = status.to_str().filter(|s| STATUSES.contains(s)) else {
            return Err(Failure::new(
                Code::Validation,
                format!("unknown status {}", repr_bytes(status.as_bytes())),
            ));
        };
        fields.values.insert("status".into(), Value::from(status));
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    use super::{TextValidity, arg_texts, encodable};

    #[test]
    fn texts_are_valid_together_only_if_each_is() {
        use TextValidity::{Invalid, Utf8};
        for (a, b, both) in [
            (Utf8, Utf8, Utf8),
            (Utf8, Invalid, Invalid),
            (Invalid, Utf8, Invalid),
            (Invalid, Invalid, Invalid),
        ] {
            assert_eq!(a.and(b), both, "{a:?} and {b:?}");
        }
        assert_eq!(
            TextValidity::default(),
            Utf8,
            "no text at all can be written"
        );
    }

    #[test]
    fn one_argument_that_is_not_utf8_makes_the_texts_unwritable() {
        let args = [
            OsStr::new("a").to_owned(),
            OsStr::from_bytes(b"b\xff").to_owned(),
        ];
        let (texts, valid) = arg_texts(&args);
        assert_eq!(texts, ["a", "b\u{fffd}"]);
        assert_eq!(valid, TextValidity::Invalid);
        assert!(encodable(valid).is_err());
        assert!(encodable(arg_texts(&args[..1]).1).is_ok());
    }
}
