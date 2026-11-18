//! The issues `list` and `ready` show: filtered, limited and laid out as rows.

use beadroll_core::event::{DONE, Field, PARENT_CHILD, status};
use beadroll_core::json::{Int, Value};
use beadroll_core::op::IntegerField;
use beadroll_core::state::{Issue, State, Target};

use std::ffi::OsString;

use super::args::{parse_priority_arg, resolve_arg};
use super::{Failure, Line, Tone, split_list};

/// The filters shared by `list` and `ready`, as given. Command-line text that is not UTF-8 matches
/// nothing (`args.rs`): the values that are UTF-8 are kept, and `unmatched` says that a filter
/// asked for something no issue can have.
#[derive(Default)]
pub(crate) struct Filters {
    pub(crate) status: Vec<String>,
    pub(crate) all: bool,
    pub(crate) deferred: bool,
    pub(crate) issue_type: Vec<String>,
    pub(crate) label: Vec<String>,
    pub(crate) priority: Option<OsString>,
    pub(crate) assignee: Option<String>,
    pub(crate) title_contains: Option<String>,
    pub(crate) id: Vec<String>,
    pub(crate) parent: Option<OsString>,
    /// A filter no issue can pass, as one whose every value was not UTF-8.
    pub(crate) unmatched: bool,
}

/// The issues the filters select, ordered by priority, creation time and key. A parent that names
/// no issue and a priority that is none are errors, never an empty answer: both are read before
/// anything is filtered.
pub(crate) fn select<'s>(st: &'s State, filters: &Filters) -> Result<Vec<&'s Issue>, Failure> {
    let statuses = split_list(&filters.status);
    // `--status all` names every status; `--all` only lifts the default that leaves done and
    // deferred issues out, so that statuses named beside it still filter
    let every_status = statuses.iter().any(|s| s == "all");
    let parent = match &filters.parent {
        Some(key) => Some(Target::Id(resolve_arg(st, key)?.uid())),
        None => None,
    };
    let types = split_list(&filters.issue_type);
    let labels = split_list(&filters.label);
    let ids = split_list(&filters.id);
    let priority = match &filters.priority {
        Some(priority) => Some(IntegerField::from(parse_priority_arg(priority)?)),
        None => None,
    };
    if filters.unmatched {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for issue in st.issues() {
        let status = issue.status();
        if !statuses.is_empty() && !every_status {
            if !statuses.iter().any(|s| s == status) {
                continue;
            }
        } else if !every_status
            && !filters.all
            && (DONE.contains(&status) || (status == status::DEFERRED && !filters.deferred))
        {
            continue;
        }
        if !filters.issue_type.is_empty() && !types.iter().any(|t| t == issue.issue_type()) {
            continue;
        }
        if !filters.label.is_empty() && !labels.iter().all(|l| issue.labels().contains(l)) {
            continue;
        }
        if (priority.as_ref()).is_some_and(|priority| issue.priority() != *priority) {
            continue;
        }
        if let Some(assignee) = filters.assignee.as_ref().filter(|a| !a.is_empty()) {
            let held = issue
                .read_field(Field::Assignee)?
                .filter(|value| **value != Value::Null);
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
            && !issue.deps().contains(parent, PARENT_CHILD)
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
    let limit = limit.as_i128();
    let len = items.len() as i128;
    let keep = if limit < 0 {
        (len + limit).max(0)
    } else {
        limit.min(len)
    };
    items.into_iter().take(keep as usize).collect()
}

/// `issues` as a listing answers them: each one's brief, in order, and each one's [`row`], or the
/// single line `empty` when there are none.
pub(crate) fn listed(st: &State, issues: &[&Issue], empty: &str) -> (Value, Vec<Line>) {
    let briefs = Value::Array(issues.iter().map(|i| Value::from(st.brief(i))).collect());
    let mut lines: Vec<Line> = issues.iter().map(|i| row(st, i)).collect();
    if lines.is_empty() {
        lines.push(empty.into());
    }
    (briefs, lines)
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
