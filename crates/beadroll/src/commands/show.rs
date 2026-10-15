//! `bead show`: issues in full, and how a human reads them.

use std::ffi::OsString;

use beadroll_core::json::{Object, Value};
use beadroll_core::text::{repr, str_of};
use beadroll_core::view::icon;
use beadroll_store::merged_into_default;

use super::{Context, Failure, Line, Outcome, Tone, need_tracker, resolve_arg, view_of};

/// The issues `ids` name in full, each with whether the commit that closed it is on the default
/// branch.
pub(crate) fn show(ctx: &Context<'_>, ids: &[OsString]) -> Result<Outcome, Failure> {
    let st = view_of(ctx.ws, ids)?;
    need_tracker(&st)?;
    // Every key is resolved first, so that the issues that depend on them are found once for all
    // of them (roll-lqjj.20); a key that names no issue still fails where it stands, after the
    // issues before it.
    let issues: Vec<_> = ids.iter().map(|key| resolve_arg(&st, key)).collect();
    let dependents = st.dependents_of(
        issues
            .iter()
            .filter_map(|issue| issue.as_ref().ok())
            .copied(),
    );
    let mut result = Vec::new();
    for issue in issues {
        let issue = issue?;
        let mut detail = st.detail_with(issue, &dependents);
        if issue.status() == "closed"
            && let Some(at) = detail.get_index_of("closed_on")
        {
            let commit = detail
                .get("closed_on")
                .and_then(|c| c.get("commit"))
                .filter(|c| c.truthy())
                .map(str_of)
                .unwrap_or_default();
            let merged = match merged_into_default(&ctx.ws.settings, &commit)? {
                Some(merged) => Value::Bool(merged),
                None => Value::from("unknown"),
            };
            detail.shift_insert(at + 1, "merged".into(), merged);
        }
        result.push(detail);
    }
    let lines: Vec<Line> = result.iter().flat_map(lines_of).collect();
    let result = Value::Array(result.into_iter().map(Value::from).collect());
    Ok(Outcome::new(result, lines))
}

/// The member `key` rendered as text, or an empty string when absent.
fn field(item: &Object, key: &str) -> String {
    item.get(key).map(str_of).unwrap_or_default()
}

/// The member `key` of an object value.
fn member<'v>(value: Option<&'v Value>, key: &str) -> Option<&'v Value> {
    value.and_then(|v| v.get(key))
}

/// The first `n` Unicode scalar values of `text`.
fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// The value as text, substituting `default` when it is falsy.
fn or(value: Option<&Value>, default: &str) -> String {
    value
        .filter(|v| v.truthy())
        .map_or_else(|| default.to_owned(), str_of)
}

/// An issue's lines, as `show` prints it for a human.
fn lines_of(item: &Object) -> Vec<Line> {
    let (first, rest) = summary(item);
    let mut lines = vec![first];
    lines.extend(rest.into_iter().map(Line::from));
    lines.extend(relations(item).into_iter().map(Line::from));
    lines
}

/// The issue's first lines: what it is, where it was worked on and closed, who holds it, and its
/// texts.
fn summary(item: &Object) -> (Line, Vec<String>) {
    let mut lines = Vec::new();
    let status = field(item, "status");
    let priority = field(item, "priority");
    let first = Line::new()
        .with(Tone::of_status(&status), icon(&status))
        .with(Tone::Plain, " ")
        .with(Tone::Id, field(item, "id"))
        .with(Tone::Plain, " · ")
        .with(Tone::Strong, field(item, "title"))
        .with(Tone::Plain, "   [")
        .with(Tone::of_priority(&priority), format!("P{priority}"))
        .with(Tone::Plain, " · ")
        .with(Tone::of_status(&status), status.to_uppercase())
        .with(Tone::Plain, "]");
    let place = item.get("where");
    if let Some(branch) = member(place, "branch").filter(|b| b.truthy()) {
        let commit = member(place, "commit").map(str_of).unwrap_or_default();
        lines.push(format!(
            "  last worked on branch {} ({})",
            str_of(branch),
            head(&commit, 12)
        ));
    }
    if let Some(closed) = item.get("closed_on").filter(|c| c.truthy()) {
        let merged = match item.get("merged") {
            Some(Value::Bool(true)) => " · merged into the default branch",
            Some(Value::Bool(false)) => " · NOT yet on the default branch",
            _ => "",
        };
        lines.push(format!(
            "  closed on {} at {}{merged}",
            or(closed.get("branch"), "detached HEAD"),
            head(&or(closed.get("commit"), ""), 12)
        ));
    }
    lines.push(format!(
        "  type {} · created {} by {}{}",
        field(item, "issue_type"),
        head(&field(item, "created_at"), 10),
        or(item.get("created_by"), "unknown"),
        match item.get("assignee").filter(|a| a.truthy()) {
            Some(assignee) => format!(" · assignee {}", str_of(assignee)),
            None => String::new(),
        }
    ));
    if let Some(held) = item.get("claim").filter(|c| c.truthy()) {
        let since = member(Some(held), "since").map(str_of).unwrap_or_default();
        let branch = member(member(Some(held), "where"), "branch").filter(|b| b.truthy());
        lines.push(format!(
            "  claimed by {} · generation {} · since {}{}",
            member(Some(held), "owner").map(str_of).unwrap_or_default(),
            member(Some(held), "generation")
                .map(str_of)
                .unwrap_or_default(),
            head(&since, 16),
            branch.map_or_else(String::new, |b| format!(" · on branch {}", str_of(b)))
        ));
    }
    for pending in list(item, "unconfirmed") {
        lines.push(format!(
            "  {} by {} awaiting the remote's acknowledgement",
            text_of(pending, "op"),
            text_of(pending, "by")
        ));
    }
    for attempt in list(item, "provisional_claims") {
        lines.push(format!(
            "  provisional claim by {} since {} (not exclusive)",
            text_of(attempt, "owner"),
            head(&text_of(attempt, "since"), 16)
        ));
    }
    for name in [
        "description",
        "acceptance_criteria",
        "notes",
        "design",
        "close_reason",
    ] {
        if let Some(value) = item.get(name).filter(|v| v.truthy()) {
            lines.push(format!("  {name}: {}", str_of(value)));
        }
    }
    if let Some(Value::Array(labels)) = item.get("labels").filter(|l| l.truthy()) {
        let labels: Vec<String> = labels.iter().map(str_of).collect();
        lines.push(format!("  labels: {}", labels.join(", ")));
    }
    (first, lines)
}

/// The issue's other lines: dependencies both ways, comments, conflicts, and the claims and ends
/// that lost.
fn relations(item: &Object) -> Vec<String> {
    let mut lines = Vec::new();
    for dep in list(item, "dependencies") {
        lines.push(format!(
            "  depends on ({}): {} [{}] {}",
            text_of(dep, "dependency_type"),
            text_of(dep, "id"),
            text_of(dep, "status"),
            text_of(dep, "title")
        ));
    }
    for dep in list(item, "unresolved_dependencies") {
        lines.push(format!(
            "  depends on ({}): {} (not an issue yet)",
            text_of(dep, "dependency_type"),
            text_of(dep, "id")
        ));
    }
    for dep in list(item, "dependents") {
        lines.push(format!(
            "  needed by ({}): {} [{}] {}",
            text_of(dep, "dependency_type"),
            text_of(dep, "id"),
            text_of(dep, "status"),
            text_of(dep, "title")
        ));
    }
    for comment in list(item, "comments") {
        lines.push(format!(
            "  [{}] {}: {}",
            head(&or(comment.get("created_at"), "?"), 16),
            or(comment.get("author"), "?"),
            text_of(comment, "text")
        ));
    }
    if let Some(Value::Object(conflicts)) = item.get("conflicts") {
        for (name, options) in conflicts.iter() {
            let options: Vec<String> = options
                .as_array()
                .unwrap_or_default()
                .iter()
                .map(|o| {
                    format!(
                        "{} by {}",
                        repr(o.get("value").unwrap_or(&Value::Null)),
                        text_of(o, "by")
                    )
                })
                .collect();
            lines.push(format!(
                "  ⚠ {name} was changed on two machines at once: {}",
                options.join(" | ")
            ));
        }
    }
    for lost in list(item, "lost_claims") {
        lines.push(format!(
            "  ! claim by {} lost to {}",
            text_of(lost, "by"),
            text_of(lost, "held_by")
        ));
    }
    for old in list(item, "stale") {
        lines.push(format!(
            "  ! ignored {} by {}: it named a claim that is no longer current",
            text_of(old, "op"),
            text_of(old, "by")
        ));
    }
    lines
}

/// The items of the list member `key`, none when it is not there.
fn list<'i>(item: &'i Object, key: &str) -> &'i [Value] {
    item.get(key).and_then(Value::as_array).unwrap_or_default()
}

/// A member of a value rendered as text, or the empty string when absent.
fn text_of(value: &Value, key: &str) -> String {
    value.get(key).map(str_of).unwrap_or_default()
}
