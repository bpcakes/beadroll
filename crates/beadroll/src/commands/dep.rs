//! `bead dep`: add or take back a dependency, list one issue's, draw its tree, or find the
//! dependency cycles.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::code::Code;
use beadroll_core::event::{BLOCKS, PARENT_CHILD};
use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::op::OpName;
use beadroll_core::query::{Edges, GraphChange, OtherParents, blocking};
use beadroll_core::state::{Edge, Issue, State, Target};
use beadroll_core::text::str_of;
use beadroll_store::view_outline;
use beadroll_sync::writing_spent;

use super::{
    Context, Failure, Line, Outcome, TextValidity, Tone, arg_text, edge_data, encodable,
    issue_not_found, need_tracker, remove_edge, resolve_arg, row,
};

/// What `dep` was asked to do, by the word that asked it: `rm` and `remove` take an edge back
/// alike, and the answer names the one given.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Add,
    Remove,
    Rm,
    List,
    Tree,
    Cycles,
}

impl Action {
    const ALL: [Action; 6] = [
        Action::Add,
        Action::Remove,
        Action::Rm,
        Action::List,
        Action::Tree,
        Action::Cycles,
    ];

    /// The word that asks for this action.
    fn word(self) -> &'static str {
        match self {
            Action::Add => "add",
            Action::Remove => "remove",
            Action::Rm => "rm",
            Action::List => "list",
            Action::Tree => "tree",
            Action::Cycles => "cycles",
        }
    }

    /// The action `word` asks for, if it is one of the words.
    fn named(word: &OsStr) -> Option<Action> {
        (Action::ALL.into_iter()).find(|action| word.as_bytes() == action.word().as_bytes())
    }
}

/// Runs `dep ACTION ...`; without a known action word, `list`.
pub(crate) fn dep(
    ctx: &Context<'_>,
    words: &[OsString],
    kind: Option<&OsStr>,
    edges: Edges,
) -> Result<Outcome, Failure> {
    let (action, words) = match words.split_first() {
        Some((first, rest)) if let Some(action) = Action::named(first) => (action, rest),
        _ => (Action::List, words),
    };
    if matches!(action, Action::Add | Action::Remove | Action::Rm) {
        return change(ctx, action, words, kind);
    }
    if kind.is_some() {
        return Err(Failure::new(
            Code::Validation,
            "--type requires bead dep add ISSUE DEPENDS_ON or bead dep remove ISSUE DEPENDS_ON",
        ));
    }
    if action == Action::Cycles && !words.is_empty() {
        return Err(Failure::new(Code::Validation, "usage: bead dep cycles"));
    }
    if action != Action::Cycles && words.len() > 1 {
        let usage = if action == Action::List {
            "usage: bead dep [list] ISSUE; to add a dependency, use: bead dep add ISSUE DEPENDS_ON"
        } else {
            "usage: bead dep tree ISSUE"
        };
        return Err(Failure::new(Code::Validation, usage));
    }
    let st = view_outline(ctx.ws)?;
    need_tracker(&st)?;
    if action == Action::Cycles {
        let cycles = st.cycles(edges);
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
        let mut result = Object::new();
        result.insert("cycles".into(), Value::from(cycles));
        result.insert("count".into(), Value::from(count));
        return Ok(Outcome::new(result, lines).answered_from(st));
    }
    let Some(key) = words.first() else {
        return Err(Failure::new(
            Code::Validation,
            format!("usage: bead dep {} ISSUE", action.word()),
        ));
    };
    let issue = resolve_arg(&st, key)?;
    if action == Action::Tree {
        let (lines, nodes) = tree(&st, issue);
        let mut result = Object::new();
        result.insert(
            "tree".into(),
            Value::Array(lines.iter().map(|l| Value::from(l.text())).collect()),
        );
        // the same rows as data: the issue's brief, its depth, and whether it was met before
        result.insert("nodes".into(), Value::Array(nodes));
        return Ok(Outcome::new(result, lines).answered_from(st));
    }
    let result = st.dependency_summary(issue);
    let entries = |name: &str| match result.get(name) {
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
/// order of its edges; an issue met again is listed but not opened again. Its lines, and the same
/// rows as data (`--json`'s `nodes`): each issue's brief with its `depth`, and `again` for one
/// met before.
fn tree(st: &State, issue: &Issue) -> (Vec<Line>, Vec<Value>) {
    let mut lines = Vec::new();
    let mut nodes = Vec::new();
    let mut seen: BTreeSet<EventId> = BTreeSet::new();
    let mut work: Vec<(&Issue, usize)> = vec![(issue, 0)];
    while let Some((node, depth)) = work.pop() {
        lines.push(Line::toned(Tone::Plain, "  ".repeat(depth)).then(row(st, node)));
        let mut data = st.brief(node);
        data.insert("depth".into(), Value::from(depth as i64));
        let again = !seen.insert(node.uid());
        if again {
            data.insert("again".into(), Value::Bool(true));
        }
        nodes.push(Value::from(data));
        if again {
            continue;
        }
        let mut edges: Vec<&Edge> = node.deps().keys().collect();
        edges.sort();
        let children: Vec<&Issue> = edges
            .into_iter()
            .filter_map(|edge| st.target(edge.to()))
            .collect();
        work.extend(children.into_iter().rev().map(|child| (child, depth + 1)));
    }
    (lines, nodes)
}

/// Whether `issue` depending on `target` with a dependency of `kind` would close a loop, judged on
/// the graph as it will be ([`State::would_wait_for_itself`]): a child inherits its ancestors'
/// prerequisites, and a loop through them is refused as a cycle is, though `dep cycles` finds none.
/// A kind that does not wait (`related`) closes none.
fn closes_a_loop(st: &State, issue: &Issue, target: &Issue, kind: &str) -> bool {
    if blocking(kind) {
        st.would_wait_for_itself(GraphChange::Prerequisite {
            issue: issue.uid(),
            target: target.uid(),
        })
    } else if kind == PARENT_CHILD {
        st.would_wait_for_itself(GraphChange::Parent {
            child: issue.uid(),
            parent: target.uid(),
            others: OtherParents::Kept,
        })
    } else {
        false
    }
}

/// `dep add|remove|rm ISSUE DEPENDS_ON`: one edge added, or the edges to it taken back; a
/// dependency an import made on a key that is not an issue yet can be taken back by that key.
fn change(
    ctx: &Context<'_>,
    action: Action,
    words: &[OsString],
    kind: Option<&OsStr>,
) -> Result<Outcome, Failure> {
    let [issue_key, target_key] = words else {
        return Err(Failure::new(
            Code::Validation,
            format!("usage: bead dep {} ISSUE DEPENDS_ON", action.word()),
        ));
    };
    // An empty `--type` is no type given, for `add` (which then adds `blocks`) and `remove` (which
    // then takes back the edges of any type) alike: no edge has the empty type.
    let (kind, validity) = match kind.filter(|kind| !kind.is_empty()) {
        Some(kind) => {
            let (text, valid) = arg_text(kind);
            (Some(text), valid)
        }
        None => (None, TextValidity::Utf8),
    };
    let ((issue_key, issue_uid, shown, shown_uid), spent) =
        writing_spent(ctx.ws, ctx.actor, |tx| {
            // read as it was before anything is emitted, without a copy (roll-lqjj.29)
            let (st, mut tx) = tx.lend()?;
            need_tracker(st)?;
            let issue = resolve_arg(st, issue_key)?;
            let uid = issue.uid().to_string();
            let (shown, target_uid) = match (action, resolve_target(st, issue, action, target_key)?)
            {
                (Action::Add, DepTarget::Issue(target)) => {
                    let kind = kind.as_deref().unwrap_or(BLOCKS);
                    let looped = closes_a_loop(st, issue, target, kind);
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
                    let target_uid = target.uid().to_string();
                    let edge = edge_data(&target_uid, kind);
                    tx.emit(OpName::DepAdd, Some(&uid), edge, None)?;
                    encodable(validity)?;
                    (target.key().to_owned(), Some(target_uid))
                }
                (_, DepTarget::Awaited(key)) => {
                    let mut data = Object::new();
                    data.insert("key".into(), Value::from(key.as_str()));
                    data.insert("type".into(), kind.clone().map_or(Value::Null, Value::from));
                    tx.emit(OpName::DepRemove, Some(&uid), data, None)?;
                    encodable(validity)?;
                    (key, None)
                }
                (_, DepTarget::Issue(target)) => {
                    let to = Target::Id(target.uid());
                    // what this machine has of it: a dependency removed elsewhere meanwhile is gone
                    // there too, so the removals still agree
                    let depends = (issue.deps().keys()).any(|edge| {
                        *edge.to() == to && kind.as_deref().is_none_or(|k| k == edge.kind())
                    });
                    if !depends {
                        let of_kind = kind
                            .as_deref()
                            .map_or_else(String::new, |k| format!(" ({k})"));
                        return Err(Failure::new(
                            Code::Validation,
                            format!(
                                "{} does not depend on {}{of_kind}",
                                issue.key(),
                                target.key()
                            ),
                        ));
                    }
                    remove_edge(&mut tx, st, issue, &to, kind.as_deref())?;
                    encodable(validity)?;
                    (target.key().to_owned(), Some(target.uid().to_string()))
                }
            };
            Ok::<_, Failure>((issue.key().to_owned(), uid, shown, target_uid))
        })?;
    let done = if action == Action::Add {
        "now depends on"
    } else {
        "no longer depends on"
    };
    let mut result = Object::new();
    result.insert("issue_id".into(), Value::from(issue_key.as_str()));
    result.insert("issue_uid".into(), Value::from(issue_uid));
    result.insert("depends_on_id".into(), Value::from(shown.as_str()));
    // an imported dependency on a key that is no issue yet has no id
    let depends_on_uid = shown_uid.map_or(Value::Null, Value::from);
    result.insert("depends_on_uid".into(), depends_on_uid);
    result.insert("action".into(), Value::from(action.word()));
    Ok(Outcome::new(result, vec![format!("{issue_key} {done} {shown}")]).wrote_in(spent))
}

/// What `dep add` or `dep remove` acts on as its target: an issue, or an imported dependency's key
/// that is no issue yet.
enum DepTarget<'s> {
    Issue(&'s Issue),
    Awaited(String),
}

/// The target `given` names for `action` on `issue`: keys first (an issue's, and for a removal an
/// awaited import's too, so no key's meaning changes), then an issue's id (`uid`).
fn resolve_target<'s>(
    st: &'s State,
    issue: &Issue,
    action: Action,
    given: &OsStr,
) -> Result<DepTarget<'s>, Failure> {
    if action == Action::Add {
        return resolve_arg(st, given).map(DepTarget::Issue);
    }
    let Some(text) = given.to_str() else {
        return Err(issue_not_found(given.as_bytes()));
    };
    if let Some(target) = st.resolve(text) {
        return Ok(DepTarget::Issue(target));
    }
    if let Some(awaited) = dangling_key(st, issue, text) {
        return Ok(DepTarget::Awaited(awaited));
    }
    st.issue_named(text)
        .map(DepTarget::Issue)
        .ok_or_else(|| issue_not_found(given.as_bytes()))
}

/// The key of the imported dependency of `issue` that `given` names, itself or with the prefix.
fn dangling_key(st: &State, issue: &Issue, given: &str) -> Option<String> {
    let prefixed = format!("{}-{given}", st.key_prefix());
    issue
        .dangling()
        .iter()
        .find_map(|d| match d.get("depends_on_id") {
            Some(Value::Str(key)) if *key == given || *key == prefixed => Some(key.clone()),
            _ => None,
        })
}
