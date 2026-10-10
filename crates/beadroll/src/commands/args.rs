//! Command-line text as Python holds it: `surrogateescape` keeps the bytes that are not UTF-8 as
//! lone surrogates, which match no key, status or name, print as `?`, and fail only when an event
//! carrying them is encoded. Here they stand as U+FFFD with a flag that fails where Python would.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::code::Code;
use beadroll_core::event::STATUSES;
use beadroll_core::json::{Int, Object, Value};
use beadroll_core::state::{Issue, State};
use beadroll_core::text::{is_space, repr_bytes};

use super::{Failure, parse_priority, read_utf8, resolve};

/// Command-line text as Python's `str` of it reads, and whether it is valid UTF-8. Python keeps
/// bytes that are not as lone surrogates, which match no key, status or name and fail only when
/// written; here they stand as U+FFFD, and `false` says to fail where Python would.
pub(crate) fn arg_text(value: &OsStr) -> (String, bool) {
    match value.to_str() {
        Some(text) => (text.to_owned(), true),
        None => (value.to_string_lossy().into_owned(), false),
    }
}

/// [`arg_text`] of each, and whether all are valid UTF-8.
pub(crate) fn arg_texts(values: &[OsString]) -> (Vec<String>, bool) {
    let mut valid = true;
    let texts = values
        .iter()
        .map(|value| {
            let (text, ok) = arg_text(value);
            valid &= ok;
            text
        })
        .collect();
    (texts, valid)
}

/// What an event will carry, and whether command-line text in it is not valid UTF-8: Python
/// finds that out only when it encodes the event, after every other check ([`encodable`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct Payload {
    pub(crate) values: Object,
    pub(crate) valid: bool,
}

impl Payload {
    pub(crate) fn new() -> Payload {
        Payload {
            values: Object::new(),
            valid: true,
        }
    }

    /// Sets `name` to command-line text, in place when it is there already.
    pub(crate) fn set_text(&mut self, name: &str, value: &OsStr) {
        let (text, valid) = arg_text(value);
        self.valid &= valid;
        self.values.insert(name.into(), Value::from(text));
    }
}

/// Python's `encode_events` of an event just emitted: text from the command line that is not
/// valid UTF-8 fails it, once every other check has passed. The transaction it fails writes
/// nothing.
pub(crate) fn encodable(valid: bool) -> Result<(), Failure> {
    if valid {
        return Ok(());
    }
    Err(Failure::new(Code::Validation, "text is not valid UTF-8"))
}

/// Python's `split_list` of command-line texts: each comma-separated item, stripped, empty ones
/// dropped, with bytes that are not UTF-8 kept as they came.
pub(crate) fn split_args(values: &[OsString]) -> Vec<OsString> {
    values
        .iter()
        .flat_map(|value| value.as_bytes().split(|b| *b == b','))
        .map(strip_arg)
        .filter(|item| !item.is_empty())
        .map(|item| OsStr::from_bytes(item).to_os_string())
        .collect()
}

/// `text.strip()` of command-line bytes: Python's whitespace off either end, a byte that is not
/// UTF-8 being none.
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

/// The issue a command-line key names; one that is not UTF-8 names none, as its lone surrogates
/// name none in Python.
pub(crate) fn resolve_arg<'s>(st: &'s State, key: &OsStr) -> Result<&'s Issue, Failure> {
    match key.to_str() {
        Some(key) => resolve(st, key),
        None => {
            let mut message = b"Issue not found: ".to_vec();
            message.extend_from_slice(key.as_bytes());
            Err(Failure::bytes(Code::IssueNotFound, message))
        }
    }
}

/// A priority given on the command line (`parse_priority` of Python's `str` of it).
pub(crate) fn parse_priority_arg(value: &OsStr) -> Result<i64, Failure> {
    match value.to_str() {
        Some(value) => parse_priority(value),
        None => Err(Failure::new(
            Code::Validation,
            format!("priority must be 0-4, got {}", repr_bytes(value.as_bytes())),
        )),
    }
}

/// The content flags `create` and `update` share (Python's `content_flags`), as given.
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

/// The fields given (Python's `field_args`): texts first, then priority, type, estimate and status.
/// A priority or status that cannot be is refused here; text that is not UTF-8 when the event is
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
    if let Some(kind) = flags.issue_type.as_deref().filter(|t| !t.is_empty()) {
        fields.set_text("issue_type", kind);
    }
    if let Some(estimate) = &flags.estimate {
        fields
            .values
            .insert("estimated_minutes".into(), Value::Int(estimate.clone()));
    }
    if let Some(status) = flags.status.as_deref().filter(|s| !s.is_empty()) {
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
