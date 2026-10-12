//! Commands: typed arguments in, a result and its human-readable lines out.

mod args;
mod claims;
mod comments;
mod create;
mod dep;
mod export;
mod history;
mod import;
mod info;
mod init;
mod line;
mod list;
mod ownership;
mod queries;
mod show;
mod sync;
mod transition;
mod update;

use std::ffi::{OsStr, OsString};
use std::io::Read as _;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::code::Code;
use beadroll_core::event::DONE;
use beadroll_core::json::{Int, Object, Value};
use beadroll_core::op::IntegerField;
use beadroll_core::state::{Issue, State};
use beadroll_core::text::{printed_text, repr_str, strip};
use beadroll_store::{
    Actor, Label, ViewError, Workspace, normalize_path, read_texts, view, view_outline,
};
use beadroll_sync::{Tx, unpublished_count};

pub(crate) use args::{
    ContentArgs, Payload, arg_text, arg_texts, encodable, field_args, resolve_arg, rpartition,
    split_args,
};
pub(crate) use claims::claims;
pub(crate) use comments::{CommentArgs, comments};
pub(crate) use create::{CreateArgs, create};
pub(crate) use dep::dep;
pub(crate) use export::export;
pub(crate) use history::{log, machines};
pub(crate) use import::import;
pub(crate) use info::info;
pub(crate) use init::{InitArgs, init};
pub(crate) use line::{Line, Tone};
pub(crate) use list::{FilterArgs, ListArgs, ReadyArgs, list, ready};
pub(crate) use ownership::{OwnershipArgs, claim, release, takeover};
pub(crate) use queries::{blocked, lint, search};
pub(crate) use show::show;
pub(crate) use sync::sync;
pub(crate) use transition::{TransitionArgs, transition};
pub(crate) use update::{UpdateArgs, update};

/// A command failure: the code and message bead reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) code: Code,
    pub(crate) message: String,
    /// The original message bytes when a command-line argument or path is not UTF-8. `message`
    /// substitutes U+FFFD; human output uses `?`, and JSON uses `\udcXX` escapes.
    pub(crate) raw: Option<Vec<u8>>,
    /// What a human reads after the message of a usage error: the usage, and where help is.
    pub(crate) after: Option<String>,
}

impl Failure {
    pub(crate) fn new(code: Code, message: impl Into<String>) -> Failure {
        Failure {
            code,
            message: message.into(),
            raw: None,
            after: None,
        }
    }

    /// A usage error: `message`, bytes that may not all be UTF-8, then `after` for a human (the
    /// usage, and where help is).
    pub(crate) fn usage(message: impl Into<Vec<u8>>, after: impl Into<String>) -> Failure {
        let after: String = after.into();
        Failure {
            after: Some(after).filter(|after| !after.is_empty()),
            ..Failure::bytes(Code::Usage, message.into())
        }
    }

    /// A failure whose message is `raw`, bytes that may not all be UTF-8.
    pub(crate) fn bytes(code: Code, raw: Vec<u8>) -> Failure {
        match String::from_utf8(raw) {
            Ok(message) => Failure::new(code, message),
            Err(error) => Failure {
                code,
                message: String::from_utf8_lossy(error.as_bytes()).into_owned(),
                raw: Some(error.into_bytes()),
                after: None,
            },
        }
    }

    /// The message for human output, replacing each byte that is not UTF-8 with `?`.
    pub(crate) fn printed(&self) -> String {
        match &self.raw {
            Some(raw) => printed_text(raw),
            None => self.message.clone(),
        }
    }
}

/// An operating-system error formatted as `{strerror}: {path}`, retaining the path's original
/// bytes.
fn os_failure(error: &beadroll_git::IoError) -> Failure {
    match &error.path {
        Some(path) => {
            let mut raw = format!("{}: ", error.strerror).into_bytes();
            raw.extend_from_slice(path.as_os_str().as_bytes());
            Failure::bytes(Code::Io, raw)
        }
        None => Failure::new(Code::Io, error.to_string()),
    }
}

impl From<beadroll_sync::Error> for Failure {
    fn from(error: beadroll_sync::Error) -> Failure {
        match error {
            beadroll_sync::Error::Store(error) => error.into(),
            error => Failure::new(error.code(), error.to_string()),
        }
    }
}

impl From<beadroll_store::Error> for Failure {
    fn from(error: beadroll_store::Error) -> Failure {
        match &error {
            beadroll_store::Error::Io(io) => os_failure(io),
            _ => Failure::new(error.code(), error.to_string()),
        }
    }
}

impl From<beadroll_git::Error> for Failure {
    fn from(error: beadroll_git::Error) -> Failure {
        Failure::new(beadroll_store::git_code(&error), error.to_string())
    }
}

impl From<beadroll_git::IoError> for Failure {
    fn from(error: beadroll_git::IoError) -> Failure {
        os_failure(&error)
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

/// What a command answers: the result `--json` prints, and the lines a human reads; or, for a
/// command that writes its own output (`export`), that output as it is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Outcome {
    pub(crate) result: Option<Value>,
    pub(crate) lines: Option<Vec<Line>>,
    pub(crate) raw: Option<String>,
    /// The command published and counted what it left unpublished itself ([`note_unpublished`]),
    /// as `init` does when it wrote through another workspace than the one it was given.
    pub(crate) settled: bool,
}

impl Outcome {
    pub(crate) fn new<L: Into<Line>>(result: impl Into<Value>, lines: Vec<L>) -> Outcome {
        Outcome {
            result: Some(result.into()),
            lines: Some(lines.into_iter().map(Into::into).collect()),
            raw: None,
            settled: false,
        }
    }

    /// Output written as it is, whatever `--json` says, with no result or lines after it.
    pub(crate) fn raw(text: String) -> Outcome {
        Outcome {
            result: None,
            lines: None,
            raw: Some(text),
            settled: false,
        }
    }
}

/// What `ws` has not published, said in `result` (`unpublished`) and `lines`, when there is any:
/// after `--offline`, a remote that did not answer, or on a store that has never published.
pub(crate) fn note_unpublished(
    ws: &Workspace,
    result: &mut Option<Value>,
    lines: &mut Option<Vec<Line>>,
) -> Result<(), Failure> {
    let Some(behind) = unpublished_count(ws)?.filter(|n| *n != Int::from(0)) else {
        return Ok(());
    };
    if let Some(Value::Object(result)) = result {
        result.insert("unpublished".into(), Value::Int(behind.clone()));
    }
    if let Some(lines) = lines {
        let plural = if behind == Int::from(1) { "" } else { "s" };
        lines.push(Line::toned(
            Tone::Warning,
            format!("{behind} event{plural} not yet published: bead sync"),
        ));
    }
    Ok(())
}

/// What every command is given: the workspace (when there is one), the actor, and the settings.
pub(crate) struct Context<'a> {
    pub(crate) ws: &'a Workspace,
    pub(crate) actor: &'a Actor,
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

/// The state as `view` gives it, with the texts of the issues `keys` name read in: what a command
/// that prints those issues whole needs, where its others' texts would only be read in vain
/// (roll-lqjj.6). Every text, when theirs cannot be read.
pub(crate) fn view_of(ws: &Workspace, keys: &[impl AsRef<OsStr>]) -> Result<State, Failure> {
    let mut st = view_outline(ws)?;
    let uids: Vec<_> = (keys.iter())
        .filter_map(|key| resolve_arg(&st, key.as_ref()).ok().map(Issue::uid))
        .collect();
    if read_texts(ws, &mut st, uids) {
        return Ok(st);
    }
    Ok(view(ws)?)
}

/// There must be a tracker here.
pub(crate) fn need_tracker(st: &State) -> Result<(), Failure> {
    if st.prefix().is_none() {
        return Err(Failure::new(
            Code::NotInitialized,
            "no tracker for this repository here yet: run `bead sync` to fetch it from its remote, or \
             `bead init` to start one if it has none",
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

/// A text argument from a file, or standard input for `-`, decoded as UTF-8 regardless of locale.
/// The path is normalised (`dir//name/.` becomes `dir/name`); `what` names it in the error as
/// given.
pub(crate) fn read_utf8(path: &OsStr, what: &OsStr) -> Result<String, Failure> {
    if path == "-" && crate::sys::stdin_closed() {
        return Err(Failure::new(
            Code::Io,
            "standard input is closed, so - cannot be read",
        ));
    }
    let raw = if path == "-" {
        let mut raw = Vec::new();
        std::io::stdin()
            .read_to_end(&mut raw)
            .map_err(|e| Failure::from(beadroll_git::IoError::bare(&e)))?;
        raw
    } else {
        let path = normalize_path(path);
        std::fs::read(&path).map_err(|e| Failure::from(beadroll_git::IoError::at(&e, &path)))?
    };
    String::from_utf8(raw).map_err(|_| {
        Failure::bytes(
            Code::Validation,
            [what.as_bytes(), b" is not valid UTF-8"].concat(),
        )
    })
}

/// Whether `me` ended the issue's last claim by closing or deleting it. That run may have died
/// before publishing, so a retry publishes as promptly as the first run.
pub(crate) fn ended_by(issue: &Issue, me: &Label) -> bool {
    issue
        .claim_history()
        .last()
        .is_some_and(|last| me.is(&last.ended_by) && matches!(last.how, "close" | "delete"))
}

/// Removes edges from `issue` to `to` that this writer has seen; `kind` set to `None` selects
/// every kind. For an imported dependency tagged with the issue's create, its key is removed too:
/// a concurrent import sorting first keeps that key, and the dependency binds to that copy after
/// merging.
pub(crate) fn remove_edge(
    tx: &mut Tx<'_>,
    st: &State,
    issue: &Issue,
    to: &str,
    kind: Option<&str>,
) -> Result<(), Failure> {
    let uid = issue.uid();
    let imported = issue.dep_tags().iter().any(|((target, k), tags)| {
        target == to && kind.is_none_or(|kind| kind == k) && tags.contains_key(&uid)
    });
    let kind = kind.map_or(Value::Null, Value::from);
    let mut data = Object::new();
    data.insert("to".into(), Value::from(to));
    data.insert("type".into(), kind.clone());
    tx.emit("dep_remove", Some(&uid.to_string()), data, None)?;
    if imported {
        let mut data = Object::new();
        data.insert("key".into(), Value::from(st.key_of(to)));
        data.insert("type".into(), kind);
        tx.emit("dep_remove", Some(&uid.to_string()), data, None)?;
    }
    Ok(())
}

/// The filters shared by `list` and `ready`, as given.
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
            && issue.priority() != IntegerField::from(parse_priority(priority)?)
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

/// The first `limit` items; a negative limit leaves that many off the end.
pub(crate) fn first<T>(items: Vec<T>, limit: &Int) -> Vec<T> {
    let limit = limit.saturating_i128();
    let len = items.len() as i128;
    let keep = if limit < 0 {
        (len + limit).max(0)
    } else {
        limit.min(len)
    };
    items.into_iter().take(keep as usize).collect()
}

/// An issue's one-line summary ([`State::row`]) as parts: its status icon, id and priority
/// toned, its type and branch dim, its assignee and any conflict marked.
pub(crate) fn row(st: &State, issue: &Issue) -> Line {
    let row = st.row(issue);
    let mut line = Line::new()
        .with(Tone::of_status(&row.status), row.icon)
        .with(Tone::Plain, " ")
        .with(Tone::Id, row.key)
        .with(Tone::Plain, " ")
        .with(
            Tone::of_priority(&row.priority),
            format!("[P{}]", row.priority),
        )
        .with(Tone::Plain, " ")
        .with(Tone::Dim, format!("[{}]", row.kind))
        .with(Tone::Plain, " - ")
        .with(Tone::Plain, row.title);
    if let Some(who) = row.assignee {
        line.push(Tone::Who, format!(" @{who}"));
    }
    if let Some(branch) = row.branch {
        line.push(Tone::Dim, format!(" ({branch})"));
    }
    if !row.disputed.is_empty() {
        line.push(
            Tone::Warning,
            format!(" ⚠ conflict: {}", row.disputed.join(", ")),
        );
    }
    line
}
