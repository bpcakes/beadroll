//! Importing a Beads JSONL export, preserving ids, timestamps, relationships and unknown fields.
//! One invalid record fails the whole import before anything is written. A dry run says what an
//! import would do, and imports nothing.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::import::dry_run::{Preview, preview, problems};
use beadroll_core::import::{Step, Target, plan_import, read_records};
use beadroll_core::json::{Object, Value};
use beadroll_core::state::State;
use beadroll_core::text::printed_text;
use beadroll_store::{Elsewhere, Workspace, elsewhere, view_outline};
use beadroll_sync::{Tx, writing};

use super::{Context, Failure, Line, Outcome, Tone, need_tracker, read_utf8};

/// The file Beads keeps its tracker in, on every branch.
const BEADS_FILE: &str = ".beads/issues.jsonl";

/// At most this many problems, and of each list of keys, are shown to a human.
const SHOWN: usize = 20;

/// `bead import PATH`: the export's records brought into this tracker, published at once; with
/// `dry_run`, what that would do, nothing imported.
pub(crate) fn import(ctx: &Context<'_>, path: &OsStr, dry_run: bool) -> Result<Outcome, Failure> {
    if dry_run {
        let state = view_outline(ctx.ws)?;
        need_tracker(&state)?;
        return answer_dry_run(ctx.ws, &state, path, Vec::new());
    }
    let count = writing(ctx.ws, ctx.actor, |tx| {
        need_tracker(tx.state()?)?;
        import_records(tx, path)
    })?;
    let mut result = Object::new();
    result.insert("imported".into(), Value::from(count as i64));
    Ok(Outcome::new(
        result,
        vec![format!("Imported {count} issues")],
    ))
}

/// Emits the events that import the export at `path` (`-`: standard input) into `tx`; how many
/// records were new.
pub(crate) fn import_records(tx: &mut Tx<'_>, path: &OsStr) -> Result<usize, Failure> {
    let content = read_utf8(path, path)?;
    // the file's name begins every message about its records, as it was given
    let records = match path.to_str() {
        Some(shown) => read_records(&content, shown)?,
        None => read_records(&content, "").map_err(|error| {
            let message = [path.as_bytes(), error.to_string().as_bytes()].concat();
            Failure::bytes(error.code(), message)
        })?,
    };
    // each form of the export let go of once the next is made: the text once read, the records
    // once planned (roll-lqjj.9)
    drop(content);
    let plan = plan_import(tx.state()?, &records)?;
    drop(records);
    let mut created: Vec<String> = Vec::new();
    for step in plan.steps {
        match step {
            Step::DepRemove { issue, data } => {
                tx.emit("dep_remove", Some(&issue.to_string()), data, None)?;
            }
            Step::Create(data) => {
                let event = tx.emit("create", None, data, None)?;
                let id = event.get("id").and_then(Value::as_str).unwrap_or_default();
                created.push(id.to_owned());
            }
            Step::DepAdd { issue, to, data } => {
                let to = match to {
                    Target::Issue(uid) => uid.to_string(),
                    Target::Created(n) => created.get(n).cloned().unwrap_or_default(),
                };
                let mut payload = Object::new();
                payload.insert("to".into(), Value::from(to));
                payload.extend(data);
                let issue = created.get(issue).cloned().unwrap_or_default();
                tx.emit("dep_add", Some(&issue), payload, None)?;
            }
        }
    }
    Ok(plan.imported)
}

/// What importing the export at `path` into `state`, `ws`'s tracker, would do: nothing imported, no
/// remote asked. `notes` say what the plan does not know, and come last.
pub(crate) fn answer_dry_run(
    ws: &Workspace,
    state: &State,
    path: &OsStr,
    notes: Vec<String>,
) -> Result<Outcome, Failure> {
    let content = read_utf8(path, path)?;
    // the file as given begins every message about its records, `?` for each byte not UTF-8
    let shown = printed_text(path.as_bytes());
    let mut result = Object::new();
    result.insert("dry_run".into(), Value::Bool(true));
    result.insert("file".into(), Value::from(shown.as_str()));
    let mut lines = vec![Line::from("Dry run: nothing imported, no remote asked.")];
    // the import's own steps decide whether it goes through
    let planned = read_records(&content, &shown)
        .and_then(|records| Ok((preview(state, &records), plan_import(state, &records)?)));
    match planned {
        Ok((would, plan)) => {
            let retracted = (plan.steps.iter())
                .filter(|step| matches!(step, Step::DepRemove { .. }))
                .count();
            result.insert("importable".into(), Value::Bool(true));
            importable(&mut result, &mut lines, &would, retracted, &shown);
        }
        Err(refused) => {
            let mut found = problems(state, &content, &shown);
            if found.is_empty() {
                found.push(refused);
            }
            result.insert("importable".into(), Value::Bool(false));
            let messages: Vec<Value> = found.iter().map(|p| Value::from(p.to_string())).collect();
            result.insert("problems".into(), Value::Array(messages));
            lines.push(Line::toned(
                Tone::Error,
                format!(
                    "Would refuse {shown}: {}, nothing would be imported",
                    counted(found.len(), "problem", "problems")
                ),
            ));
            for problem in found.iter().take(SHOWN) {
                lines.push(Line::toned(Tone::Error, format!("  {problem}")));
            }
            if found.len() > SHOWN {
                lines.push(Line::from(format!(
                    "  and {} more (--json lists all)",
                    found.len() - SHOWN
                )));
            }
        }
    }
    let changes = elsewhere(&ws.settings, BEADS_FILE)?;
    report_elsewhere(&mut result, &mut lines, &changes);
    lines.extend(notes.into_iter().map(Line::from));
    Ok(Outcome::new(result, lines))
}

/// The answer of a dry run whose import would go through.
fn importable(
    result: &mut Object,
    lines: &mut Vec<Line>,
    would: &Preview,
    retracted: usize,
    shown: &str,
) {
    let new = would.records - would.known;
    for (name, count) in [
        ("records", would.records),
        ("new", new),
        ("known", would.known),
    ] {
        result.insert(name.into(), Value::from(count as i64));
    }
    result.insert("retracted".into(), Value::from(retracted as i64));
    let statuses: Object = (would.statuses.iter())
        .map(|(status, count)| (status.clone(), Value::from(*count as i64)))
        .collect();
    result.insert("statuses".into(), Value::from(statuses));
    let keys =
        |keys: &[String]| Value::Array(keys.iter().map(|k| Value::from(k.as_str())).collect());
    result.insert("unowned".into(), keys(&would.unowned));
    result.insert("waiting".into(), keys(&would.waiting));
    let mut said = format!(
        "Would import {} from {shown}",
        counted(new, "issue", "issues")
    );
    if would.known > 0 {
        said.push_str(&format!(
            " ({} already here, left as they are)",
            would.known
        ));
    }
    lines.push(Line::from(said));
    if !would.statuses.is_empty() {
        let counts: Vec<String> = (would.statuses.iter())
            .map(|(status, count)| format!("{status} {count}"))
            .collect();
        lines.push(Line::from(format!("  {}", counts.join(", "))));
    }
    if retracted > 0 {
        lines.push(Line::from(format!(
            "Would take back {} the issues already here no longer list",
            counted(retracted, "dependency", "dependencies")
        )));
    }
    if !would.unowned.is_empty() {
        lines.push(Line::toned(
            Tone::Warning,
            format!(
                "No owner for {} (a claim, in bead): claim or reopen each after the import: {}",
                counted(
                    would.unowned.len(),
                    "in_progress issue",
                    "in_progress issues"
                ),
                listed(&would.unowned)
            ),
        ));
    }
    if !would.waiting.is_empty() {
        lines.push(Line::from(format!(
            "Depended on but in neither the tracker nor the file, kept until such an issue \
             arrives: {}, {}",
            counted(would.waiting.len(), "key", "keys"),
            listed(&would.waiting)
        )));
    }
}

/// Where the tracker file has changes the checkout lacks, in the answer.
fn report_elsewhere(result: &mut Object, lines: &mut Vec<Line>, changes: &Elsewhere) {
    let refs: Vec<Value> = (changes.refs.iter())
        .map(|(name, commits)| {
            let mut entry = Object::new();
            entry.insert("ref".into(), Value::from(name.as_str()));
            entry.insert("commits".into(), Value::from(*commits as i64));
            Value::from(entry)
        })
        .collect();
    let worktrees: Vec<Value> = (changes.worktrees.iter())
        .map(|tree| Value::from(tree.as_str()))
        .collect();
    let mut found = Object::new();
    found.insert("refs".into(), Value::Array(refs));
    found.insert("worktrees".into(), Value::Array(worktrees));
    result.insert("elsewhere".into(), Value::from(found));
    if changes.refs.is_empty() && changes.worktrees.is_empty() {
        return;
    }
    lines.push(Line::toned(
        Tone::Warning,
        format!(
            "{BEADS_FILE} has changes this checkout lacks: bring them here and export again first, \
             or the import misses them (a later import adds issues, never newer copies of them)"
        ),
    ));
    for (name, commits) in &changes.refs {
        lines.push(Line::from(format!(
            "  {name}: {} touching it that HEAD has not",
            counted(*commits as usize, "commit", "commits")
        )));
    }
    for tree in &changes.worktrees {
        lines.push(Line::from(format!("  {tree}: changes to it not committed")));
    }
}

/// `count` and the noun for it, `one` or `many`.
fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// `keys`, at most [`SHOWN`] of them, then how many more.
fn listed(keys: &[String]) -> String {
    let mut said = keys
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if keys.len() > SHOWN {
        said.push_str(&format!(" and {} more", keys.len() - SHOWN));
    }
    said
}
