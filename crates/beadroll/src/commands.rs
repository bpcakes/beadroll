//! The commands (Python's `commands.py`): typed arguments in, a result and its human lines out.

mod create;
mod import;
mod init;
mod list;
mod show;
mod sync;

use std::ffi::{OsStr, OsString};
use std::io::Read as _;

use beadroll_core::code::Code;
use beadroll_core::event::{DONE, STATUSES};
use beadroll_core::json::{Object, Value};
use beadroll_core::op::PyInt;
use beadroll_core::state::{Issue, State};
use beadroll_core::text::{repr_str, strip};
use beadroll_store::{ViewError, Workspace};

pub(crate) use create::{CreateArgs, create};
pub(crate) use init::{InitArgs, init};
pub(crate) use list::{FilterArgs, ListArgs, ReadyArgs, list, ready};
pub(crate) use show::show;
pub(crate) use sync::sync;

/// A command's failure: the code and message bead reports (Python's `BeadError`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) code: Code,
    pub(crate) message: String,
}

impl Failure {
    pub(crate) fn new(code: Code, message: impl Into<String>) -> Failure {
        Failure {
            code,
            message: message.into(),
        }
    }
}

impl From<beadroll_sync::Error> for Failure {
    fn from(error: beadroll_sync::Error) -> Failure {
        Failure::new(error.code(), error.to_string())
    }
}

impl From<beadroll_store::Error> for Failure {
    fn from(error: beadroll_store::Error) -> Failure {
        Failure::new(error.code(), error.to_string())
    }
}

impl From<beadroll_git::Error> for Failure {
    fn from(error: beadroll_git::Error) -> Failure {
        Failure::new(beadroll_store::git_code(&error), error.to_string())
    }
}

impl From<beadroll_git::IoError> for Failure {
    fn from(error: beadroll_git::IoError) -> Failure {
        Failure::new(Code::Io, error.to_string())
    }
}

impl From<ViewError> for Failure {
    fn from(error: ViewError) -> Failure {
        match error {
            ViewError::Store(error) => error.into(),
            ViewError::KeySpace(error) => Failure::new(error.code(), error.to_string()),
        }
    }
}

impl From<beadroll_core::key::MintError> for Failure {
    fn from(error: beadroll_core::key::MintError) -> Failure {
        Failure::new(error.code(), error.to_string())
    }
}

impl From<beadroll_core::import::ImportError> for Failure {
    fn from(error: beadroll_core::import::ImportError) -> Failure {
        Failure::new(error.code(), error.to_string())
    }
}

/// What a command answers: the result `--json` prints, and the lines a human reads (`None` for a
/// command that wrote its own output).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Outcome {
    pub(crate) result: Option<Value>,
    pub(crate) lines: Option<Vec<String>>,
}

impl Outcome {
    pub(crate) fn new(result: impl Into<Value>, lines: Vec<String>) -> Outcome {
        Outcome {
            result: Some(result.into()),
            lines: Some(lines),
        }
    }
}

/// What every command is given: the workspace (when there is one), the actor, and the settings.
pub(crate) struct Context<'a> {
    pub(crate) ws: &'a Workspace,
    pub(crate) actor: &'a Value,
}

/// An argument as text: text that is not valid UTF-8 cannot be written to an event.
pub(crate) fn text(value: &OsStr) -> Result<String, Failure> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| Failure::new(Code::Validation, "text is not valid UTF-8"))
}

/// Each argument as text.
pub(crate) fn texts(values: &[OsString]) -> Result<Vec<String>, Failure> {
    values.iter().map(|v| text(v)).collect()
}

/// There must be a tracker here.
pub(crate) fn need_tracker(st: &State) -> Result<(), Failure> {
    if st.prefix().is_none() {
        return Err(Failure::new(
            Code::NotInitialized,
            "no tracker for this repository yet: run `bead init` or `bead sync`",
        ));
    }
    Ok(())
}

/// The issue a key (or a bare suffix) names.
pub(crate) fn resolve<'s>(st: &'s State, key: &str) -> Result<&'s Issue, Failure> {
    st.resolve(key)
        .ok_or_else(|| Failure::new(Code::IssueNotFound, format!("Issue not found: {key}")))
}

/// A priority given as `0`-`4` or `P0`-`P4`.
pub(crate) fn parse_priority(value: &str) -> Result<i64, Failure> {
    let digits = value.strip_prefix(['P', 'p']).unwrap_or(value);
    match digits.as_bytes() {
        [d @ b'0'..=b'4'] => Ok(i64::from(d - b'0')),
        _ => Err(Failure::new(
            Code::Validation,
            format!("priority must be 0-4, got {}", repr_str(value)),
        )),
    }
}

/// Comma-separated lists, flattened, each item stripped, empty ones dropped.
pub(crate) fn split_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|value| value.split(','))
        .map(strip)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A text argument from a file (`-` for standard input), decoded as UTF-8 whatever the locale says.
pub(crate) fn read_utf8(path: &OsStr, what: &str) -> Result<String, Failure> {
    let raw = if path == "-" {
        let mut raw = Vec::new();
        std::io::stdin()
            .read_to_end(&mut raw)
            .map_err(|e| Failure::from(beadroll_git::IoError::bare(&e)))?;
        raw
    } else {
        std::fs::read(path).map_err(|e| Failure::from(beadroll_git::IoError::at(&e, path)))?
    };
    String::from_utf8(raw)
        .map_err(|_| Failure::new(Code::Validation, format!("{what} is not valid UTF-8")))
}

/// The fields `create` and `update` set from their flags, in Python's order.
pub(crate) struct FieldFlags<'a> {
    pub(crate) title: Option<&'a OsStr>,
    pub(crate) description: Option<&'a OsStr>,
    pub(crate) description_file: Option<&'a OsStr>,
    pub(crate) acceptance_criteria: Option<&'a OsStr>,
    pub(crate) notes: Option<&'a OsStr>,
    pub(crate) design: Option<&'a OsStr>,
    pub(crate) assignee: Option<&'a OsStr>,
    pub(crate) external_ref: Option<&'a OsStr>,
    pub(crate) priority: Option<&'a OsStr>,
    pub(crate) issue_type: Option<&'a OsStr>,
    pub(crate) estimate: Option<i128>,
    pub(crate) status: Option<&'a OsStr>,
}

/// The fields given (Python's `field_args`): texts first, then priority, type, estimate and status.
pub(crate) fn field_args(flags: &FieldFlags<'_>) -> Result<Object, Failure> {
    let mut fields = Object::new();
    let texts = [
        ("title", flags.title),
        ("description", flags.description),
        ("acceptance_criteria", flags.acceptance_criteria),
        ("notes", flags.notes),
        ("design", flags.design),
        ("assignee", flags.assignee),
        ("external_ref", flags.external_ref),
    ];
    for (name, value) in texts {
        let value = match (name, flags.description_file) {
            ("description", Some(path)) if !path.is_empty() => Some(read_utf8(path, name)?),
            _ => value.map(text).transpose()?,
        };
        if let Some(value) = value {
            fields.insert(name.into(), Value::from(value));
        }
    }
    if let Some(priority) = flags.priority {
        fields.insert(
            "priority".into(),
            Value::from(parse_priority(&text(priority)?)?),
        );
    }
    if let Some(kind) = flags.issue_type.filter(|t| !t.is_empty()) {
        fields.insert("issue_type".into(), Value::from(text(kind)?));
    }
    if let Some(estimate) = flags.estimate {
        fields.insert("estimated_minutes".into(), int_value(estimate));
    }
    if let Some(status) = flags.status.filter(|s| !s.is_empty()) {
        let status = text(status)?;
        if !STATUSES.contains(&status.as_str()) {
            return Err(Failure::new(
                Code::Validation,
                format!("unknown status {}", repr_str(&status)),
            ));
        }
        fields.insert("status".into(), Value::from(status));
    }
    Ok(fields)
}

/// A whole number argument as an event value.
pub(crate) fn int_value(value: i128) -> Value {
    match i64::try_from(value) {
        Ok(small) => Value::from(small),
        Err(_) => beadroll_core::json::from_text(&value.to_string()).unwrap_or(Value::Null),
    }
}

/// The filters `list` and `ready` share (Python's `select`), as given.
#[derive(Default)]
pub(crate) struct Filters {
    pub(crate) status: Vec<String>,
    pub(crate) all: bool,
    pub(crate) deferred: bool,
    pub(crate) issue_type: Vec<String>,
    pub(crate) label: Vec<String>,
    pub(crate) priority: Option<String>,
    pub(crate) assignee: Option<String>,
    pub(crate) title_contains: Option<String>,
    pub(crate) id: Vec<String>,
    pub(crate) parent: Option<String>,
}

/// The issues the filters select, ordered by priority, creation time and key. A parent that names
/// no issue is an error, never an empty answer: it is resolved before anything is filtered.
pub(crate) fn select<'s>(st: &'s State, filters: &Filters) -> Result<Vec<&'s Issue>, Failure> {
    let statuses = split_list(&filters.status);
    let want_all = filters.all || statuses.iter().any(|s| s == "all");
    let parent = match &filters.parent {
        Some(key) => Some(resolve(st, key)?.uid().to_string()),
        None => None,
    };
    let types = split_list(&filters.issue_type);
    let labels = split_list(&filters.label);
    let ids = split_list(&filters.id);
    let mut items = Vec::new();
    for issue in st.issues() {
        let status = issue.status();
        if !statuses.is_empty() && !want_all {
            if !statuses.iter().any(|s| s == status) {
                continue;
            }
        } else if !want_all
            && (DONE.contains(&status) || (status == "deferred" && !filters.deferred))
        {
            continue;
        }
        if !filters.issue_type.is_empty() && !types.iter().any(|t| t == issue.issue_type()) {
            continue;
        }
        if !filters.label.is_empty() && !labels.iter().all(|l| issue.labels().contains(l)) {
            continue;
        }
        if let Some(priority) = &filters.priority
            && issue.priority() != PyInt::from(parse_priority(priority)?)
        {
            continue;
        }
        if let Some(assignee) = filters.assignee.as_ref().filter(|a| !a.is_empty()) {
            let held = issue.get(beadroll_core::event::Field::Assignee);
            if held != Some(&Value::from(assignee.as_str())) {
                continue;
            }
        }
        if let Some(needle) = filters.title_contains.as_ref().filter(|n| !n.is_empty())
            && !issue
                .title()
                .to_lowercase()
                .contains(&needle.to_lowercase())
        {
            continue;
        }
        if !filters.id.is_empty() && !ids.iter().any(|id| id == issue.key()) {
            continue;
        }
        if let Some(parent) = &parent
            && !issue
                .deps()
                .contains_key(&(parent.clone(), "parent-child".to_owned()))
        {
            continue;
        }
        items.push(issue);
    }
    beadroll_core::query::sort_for_listing(&mut items);
    Ok(items)
}

/// Python's `items[:limit]` for a list: a negative limit leaves that many off the end.
pub(crate) fn first<T>(items: Vec<T>, limit: i128) -> Vec<T> {
    let len = items.len() as i128;
    let keep = if limit < 0 {
        (len + limit).max(0)
    } else {
        limit.min(len)
    };
    items.into_iter().take(keep as usize).collect()
}
