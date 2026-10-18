//! How `bead recap` lays a tracker's activity out for a person (`--format`): one line per event, one
//! per issue, or every event in full. `--json` is the same whatever the layout.

use std::collections::BTreeMap;

use beadroll_core::journal::Event;
use beadroll_core::json::Object;
use beadroll_core::op::Op;
use beadroll_core::state::{Issue, State};
use beadroll_core::text::str_of;
use beadroll_core::view::icon;

use crate::commands::{Line, Tone};

/// How the human output lays the activity out (`--format`); `--json` always lists every event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    /// One line per event, as `log` prints it, with its issue's title.
    Events,
    /// One line per issue: its status, what happened to it, who wrote last and when.
    Issues,
    /// Every event with its summary, machine, agent, thread, branch and commit.
    Full,
}

impl Layout {
    /// The layout `name` names: `events`, `issues` or `full`.
    pub(crate) fn named(name: &str) -> Option<Layout> {
        match name {
            "events" => Some(Layout::Events),
            "issues" => Some(Layout::Issues),
            "full" => Some(Layout::Full),
            _ => None,
        }
    }
}

/// The lines of a tracker's activity laid out as `layout`: `rows`, each event shown with the row
/// and the line `log` made of it, in causal order, and `st`, the tracker's state now.
pub(super) fn activity(layout: Layout, rows: &[(&Event, Object, Line)], st: &State) -> Vec<Line> {
    match layout {
        _ if rows.is_empty() => vec!["  No new activity".into()],
        Layout::Events => (rows.iter())
            .map(|(_, _, line)| Line::toned(Tone::Plain, "  ").then(line.clone()))
            .collect(),
        Layout::Issues => issue_lines(rows, st),
        Layout::Full => (rows.iter())
            .flat_map(|(_, row, _)| full_lines(row))
            .collect(),
    }
}

/// A time as `log` prints it: to the second.
pub(super) fn time(at: &str) -> String {
    at.chars().take(19).collect()
}

/// One line for each issue the events of `rows` are about, in the order of each one's last event:
/// its status now, key and title; what happened to it; and who wrote its last event, where and when.
fn issue_lines(rows: &[(&Event, Object, Line)], st: &State) -> Vec<Line> {
    let mut issues: Vec<(Option<&str>, Vec<usize>)> = Vec::new();
    let mut found: BTreeMap<Option<&str>, usize> = BTreeMap::new();
    for (at, (event, _, _)) in rows.iter().enumerate() {
        let index = *found.entry(event.issue()).or_insert_with(|| {
            issues.push((event.issue(), Vec::new()));
            issues.len() - 1
        });
        if let Some((_, ats)) = issues.get_mut(index) {
            ats.push(at);
        }
    }
    issues.sort_by_key(|(_, ats)| ats.last().copied());
    let mut lines = Vec::with_capacity(issues.len());
    for (uid, ats) in issues {
        let events: Vec<&Event> = (ats.iter().filter_map(|at| rows.get(*at)))
            .map(|(event, _, _)| *event)
            .collect();
        let issue = uid.and_then(|uid| st.issue_named(uid));
        let status = issue.map_or("", Issue::status);
        let key = uid.map_or("-", |uid| st.key_of(uid));
        let mut line = Line::toned(Tone::Plain, "  ")
            .with(Tone::of_status(status), icon(status))
            .with(Tone::Plain, " ")
            .with(Tone::Id, key)
            .with(Tone::Plain, format!("  {}", issue.map_or("", Issue::title)))
            .with(Tone::Plain, format!("  {}", happened(&events)));
        if let Some((_, row, _)) = ats.last().and_then(|at| rows.get(*at)) {
            line.push(Tone::Who, format!("  {}", who(row)));
            line.push(Tone::Dim, format!("  {}", time(&text(row, "at"))));
        }
        lines.push(line);
    }
    lines
}

/// What `events`, all about one issue, did to it, in order: its changes of state, then how many
/// comments, edits and other events.
fn happened(events: &[&Event]) -> String {
    let (mut changes, mut comments, mut edits, mut others) = (Vec::new(), 0, 0, 0);
    for event in events {
        let change = match event.op() {
            Op::Create(create) if create.imported => "imported",
            Op::Create(_) => "created",
            Op::Claim(claim) if claim.mode.as_deref() == Some("offline") => "claimed provisionally",
            Op::Claim(claim) if claim.takeover.as_deref().is_some_and(|t| !t.is_empty()) => {
                "taken over"
            }
            Op::Claim(_) => "claimed",
            Op::Release(_) => "released",
            Op::Close(_) => "closed",
            Op::Reopen => "reopened",
            Op::Delete(_) => "deleted",
            Op::Comment { .. } => {
                comments += 1;
                continue;
            }
            Op::Set(_)
            | Op::DepAdd { .. }
            | Op::DepRemove { .. }
            | Op::LabelAdd(_)
            | Op::LabelRemove(_)
            | Op::LabelSet(_) => {
                edits += 1;
                continue;
            }
            _ => {
                others += 1;
                continue;
            }
        };
        if changes.last() != Some(&change) {
            changes.push(change);
        }
    }
    let mut parts: Vec<String> = changes.into_iter().map(str::to_owned).collect();
    for (n, what) in [
        (comments, "comment"),
        (edits, "edit"),
        (others, "other event"),
    ] {
        match n {
            0 => {}
            1 => parts.push(format!("1 {what}")),
            n => parts.push(format!("{n} {what}s")),
        }
    }
    parts.join(", ")
}

/// Who wrote the event of `row`, a row `log` made of it, as `log` names them: the machine, then the
/// actor's name, else its agent (`?` when it has none) and the start of its thread.
fn who(row: &Object) -> String {
    let named = row.get("name").filter(|name| name.truthy()).map(str_of);
    let who = named.unwrap_or_else(|| {
        let agent = (row.get("agent").filter(|agent| agent.truthy()))
            .map_or_else(|| "?".to_owned(), str_of);
        let thread: String = text(row, "thread").chars().take(8).collect();
        format!("{agent} {thread}")
    });
    format!("{} {who}", text(row, "machine"))
}

/// The member `key` of `row` as text, `-` when it is empty or absent.
fn text(row: &Object, key: &str) -> String {
    (row.get(key).filter(|value| value.truthy())).map_or_else(|| "-".to_owned(), str_of)
}

/// An event in full: when, its issue and title; what it did; the machine that wrote it (and its
/// local name), the agent, thread and actor's name; and the branch and commit it was written on.
fn full_lines(row: &Object) -> Vec<Line> {
    let machine = text(row, "machine_id");
    let alias = match text(row, "machine") {
        alias if alias != machine => format!(" ({alias})"),
        _ => String::new(),
    };
    vec![
        Line::new()
            .with(Tone::Dim, format!("  {}  ", text(row, "at")))
            .with(Tone::Id, text(row, "issue"))
            .with(Tone::Plain, format!("  {}", text(row, "title"))),
        Line::from(format!("    {}", text(row, "summary"))),
        Line::toned(
            Tone::Who,
            format!(
                "    machine={machine}{alias} agent={} thread={} name={}",
                text(row, "agent"),
                text(row, "thread"),
                text(row, "name")
            ),
        ),
        Line::from(format!(
            "    branch={} commit={}",
            text(row, "branch"),
            text(row, "commit")
        )),
    ]
}
