//! `bead dep`: add or take back a dependency, list one issue's, draw its tree, or find the
//! dependency cycles.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::code::Code;
use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::query::{Edges, blocking};
use beadroll_core::state::{Issue, State};
use beadroll_core::text::str_of;
use beadroll_store::view_outline;
use beadroll_sync::writing;

use super::{
    Context, Failure, Line, Outcome, Tone, arg_text, encodable, need_tracker, remove_edge,
    resolve_arg, row,
};

/// Runs `dep ACTION ...`; without a known action word, `list`.
pub(crate) fn dep(
    ctx: &Context<'_>,
    words: &[OsString],
    kind: Option<&OsStr>,
    blocking_only: bool,
) -> Result<Outcome, Failure> {
    let known = ["add", "remove", "rm", "list", "tree", "cycles"];
    let (action, words) = match words.split_first() {
        Some((first, rest)) if known.iter().any(|k| first.as_bytes() == k.as_bytes()) => {
            (first.to_string_lossy().into_owned(), rest)
        }
        _ => ("list".to_owned(), words),
    };
    if matches!(action.as_str(), "add" | "remove" | "rm") {
        return change(ctx, &action, words, kind);
    }
    let st = view_outline(ctx.ws)?;
    need_tracker(&st)?;
    if action == "cycles" {
        let cycles = st.cycles(if blocking_only {
            Edges::Blocking
        } else {
            Edges::WaitsFor
        });
        let mut lines: Vec<String> = cycles
            .iter()
            .map(|cycle| {
                let mut around = cycle.clone();
                around.extend(cycle.first().cloned());
                around.join(" -> ")
            })
            .collect();
        if lines.is_empty() {
            lines.push("No cycles".to_owned());
        }
        let count = cycles.len() as i64;
        let cycles = cycles
            .into_iter()
            .map(|cycle| Value::Array(cycle.into_iter().map(Value::from).collect()))
            .collect();
        let mut result = Object::new();
        result.insert("cycles".into(), Value::Array(cycles));
        result.insert("count".into(), Value::from(count));
        return Ok(Outcome::new(result, lines).answered_from(st));
    }
    let Some(key) = words.first() else {
        return Err(Failure::new(
            Code::Validation,
            format!("usage: bead dep {action} ISSUE"),
        ));
    };
    let issue = resolve_arg(&st, key)?;
    if action == "tree" {
        let lines = tree(&st, issue);
        let mut result = Object::new();
        result.insert(
            "tree".into(),
            Value::Array(lines.iter().map(|l| Value::from(l.text())).collect()),
        );
        return Ok(Outcome::new(result, lines).answered_from(st));
    }
    let info = st.detail(issue);
    let mut result = Object::new();
    for name in ["dependencies", "dependents", "unresolved_dependencies"] {
        if let Some(value) = info.get(name) {
            result.insert(name.into(), value.clone());
        }
    }
    let entries = |name: &str| match info.get(name) {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let shown = |item: &Value, key: &str| item.get(key).map(str_of).unwrap_or_default();
    let mut lines: Vec<String> = Vec::new();
    for d in entries("dependencies") {
        lines.push(format!(
            "depends on {} ({})",
            shown(&d, "id"),
            shown(&d, "dependency_type")
        ));
    }
    for d in entries("unresolved_dependencies") {
        lines.push(format!(
            "depends on {} ({}, not an issue yet)",
            shown(&d, "id"),
            shown(&d, "dependency_type")
        ));
    }
    for d in entries("dependents") {
        lines.push(format!(
            "needed by {} ({})",
            shown(&d, "id"),
            shown(&d, "dependency_type")
        ));
    }
    if lines.is_empty() {
        lines.push("No dependencies".to_owned());
    }
    Ok(Outcome::new(result, lines).answered_from(st))
}

/// The tree beneath `issue`: each issue it depends on, indented under it, depth first in the
/// order of its edges; an issue met again is listed but not opened again.
fn tree(st: &State, issue: &Issue) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut seen: BTreeSet<EventId> = BTreeSet::new();
    let mut work: Vec<(&Issue, usize)> = vec![(issue, 0)];
    while let Some((node, depth)) = work.pop() {
        lines.push(Line::toned(Tone::Plain, "  ".repeat(depth)).then(row(st, node)));
        if !seen.insert(node.uid()) {
            continue;
        }
        let mut edges: Vec<&(String, String)> = node.deps().keys().collect();
        edges.sort();
        let children: Vec<&Issue> = edges
            .into_iter()
            .filter_map(|(to, _)| st.issue_named(to))
            .collect();
        work.extend(children.into_iter().rev().map(|child| (child, depth + 1)));
    }
    lines
}

/// `dep add|remove|rm ISSUE DEPENDS_ON`: one edge added, or the edges to it taken back; a
/// dependency an import made on a key that is not an issue yet can be taken back by that key.
fn change(
    ctx: &Context<'_>,
    action: &str,
    words: &[OsString],
    kind: Option<&OsStr>,
) -> Result<Outcome, Failure> {
    let [issue_key, target_key] = words else {
        return Err(Failure::new(
            Code::Validation,
            format!("usage: bead dep {action} ISSUE DEPENDS_ON"),
        ));
    };
    let (kind, kind_valid) = match kind {
        Some(kind) => {
            let (text, valid) = arg_text(kind);
            (Some(text), valid)
        }
        None => (None, true),
    };
    let (issue_key, shown) = writing(ctx.ws, ctx.actor, |tx| {
        // read as it was before anything is emitted, without a copy (roll-lqjj.29)
        let (st, mut tx) = tx.lend()?;
        need_tracker(st)?;
        let issue = resolve_arg(st, issue_key)?;
        let uid = issue.uid().to_string();
        let target = match resolve_arg(st, target_key) {
            Ok(target) => Some(target),
            Err(failure) => {
                if action == "add" || dangling_key(st, issue, target_key).is_none() {
                    return Err(failure);
                }
                // an imported dependency on a key that is not an issue yet: it can still be
                // taken back
                None
            }
        };
        let shown = match (action, target) {
            ("add", Some(target)) => {
                let kind = kind
                    .as_deref()
                    .filter(|k| !k.is_empty())
                    .unwrap_or("blocks");
                let looped = if blocking(kind) {
                    st.reaches(target.uid(), issue.uid())
                } else if kind == "parent-child" {
                    st.reaches(issue.uid(), target.uid())
                } else {
                    false
                };
                if issue.uid() == target.uid() || looped {
                    return Err(Failure::new(
                        Code::Cycle,
                        format!(
                            "{} -> {} would create a dependency cycle",
                            issue.key(),
                            target.key()
                        ),
                    ));
                }
                let mut data = Object::new();
                data.insert("to".into(), Value::from(target.uid().to_string()));
                data.insert("type".into(), Value::from(kind));
                tx.emit("dep_add", Some(&uid), data, None)?;
                encodable(kind_valid)?;
                target.key().to_owned()
            }
            (_, None) => {
                let key = dangling_key(st, issue, target_key).unwrap_or_default();
                let mut data = Object::new();
                data.insert("key".into(), Value::from(key.as_str()));
                data.insert("type".into(), kind.clone().map_or(Value::Null, Value::from));
                tx.emit("dep_remove", Some(&uid), data, None)?;
                encodable(kind_valid)?;
                key
            }
            (_, Some(target)) => {
                remove_edge(
                    &mut tx,
                    st,
                    issue,
                    &target.uid().to_string(),
                    kind.as_deref(),
                )?;
                encodable(kind_valid)?;
                target.key().to_owned()
            }
        };
        Ok::<_, Failure>((issue.key().to_owned(), shown))
    })?;
    let done = if action == "add" {
        "now depends on"
    } else {
        "no longer depends on"
    };
    let mut result = Object::new();
    result.insert("issue_id".into(), Value::from(issue_key.as_str()));
    result.insert("depends_on_id".into(), Value::from(shown.as_str()));
    result.insert("action".into(), Value::from(action));
    Ok(Outcome::new(
        result,
        vec![format!("{issue_key} {done} {shown}")],
    ))
}

/// The key of the imported dependency of `issue` that `given` names, itself or with the prefix.
fn dangling_key(st: &State, issue: &Issue, given: &OsStr) -> Option<String> {
    let given = given.to_str()?;
    let prefix = st.prefix().map_or_else(|| "None".to_owned(), str_of);
    let prefixed = format!("{prefix}-{given}");
    issue
        .dangling()
        .iter()
        .find_map(|d| match d.get("depends_on_id") {
            Some(Value::Str(key)) if *key == given || *key == prefixed => Some(key.clone()),
            _ => None,
        })
}
