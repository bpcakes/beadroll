//! Importing a Beads JSONL export, preserving ids, timestamps, relationships and unknown fields.
//! One invalid record fails the whole import before anything is written. A dry run says what an
//! import would do, and imports nothing.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::code::Code;
use beadroll_core::import::dry_run::{Preview, preview, problems};
use beadroll_core::import::{Step, Target, plan_import, read_records};
use beadroll_core::json::{Object, Value};
use beadroll_core::op::OpName;
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_core::text::printed_text;
use beadroll_store::{Elsewhere, ViewError, Workspace, elsewhere, load_journal};
use beadroll_sync::{Tx, writing_spent};

use super::{Context, Failure, Line, Outcome, Tone, counted, need_tracker, read_utf8};
use crate::commands::ImportMode;

/// The file Beads keeps its tracker in, on every branch.
const BEADS_FILE: &str = ".beads/issues.jsonl";

/// At most this many problems, and of each list of keys, are shown to a human.
const SHOWN: usize = 20;

/// `bead import PATH`: the export's records brought into this tracker, published at once; when
/// `mode` is `ImportMode::DryRun`, what that would do, nothing imported.
pub(crate) fn import(
    ctx: &Context<'_>,
    path: &OsStr,
    mode: ImportMode,
) -> Result<Outcome, Failure> {
    if mode == ImportMode::DryRun {
        let state = read_only(ctx.ws)?;
        need_tracker(&state)?;
        return Ok(answer_dry_run(ctx.ws, &state, path, Vec::new(), None)?.answered_from(state));
    }
    let (imported, spent) = writing_spent(ctx.ws, ctx.actor, |tx| {
        need_tracker(tx.state()?)?;
        import_records(tx, path)
    })?;
    let mut result = Object::new();
    result.insert("imported".into(), Value::from(imported.issues as i64));
    let line = imported_line(&mut result, &imported);
    Ok(Outcome::new(result, vec![line]).wrote_in(spent))
}

/// What an import did: the issues it made, and the dependencies it took back, which a record
/// imported again without them retracts.
pub(crate) struct Imported {
    pub(crate) issues: usize,
    pub(crate) taken_back: usize,
}

/// The line that says what `imported` did, `taken_back` in `result` too when it took any back:
/// a re-import that only retracts writes, and says so, never `Imported 0 issues` alone.
pub(crate) fn imported_line(result: &mut Object, imported: &Imported) -> String {
    let mut line = format!("Imported {} issues", imported.issues);
    if imported.taken_back > 0 {
        result.insert("taken_back".into(), Value::from(imported.taken_back as i64));
        line.push_str(&format!(
            ", took back {}",
            counted(imported.taken_back, "dependency", "dependencies")
        ));
    }
    line
}

/// Emits the events that import the export at `path` (`-`: standard input) into `tx`; how many
/// records were new.
/// The prefix every record of the export at `path` is keyed under (`bd-1`, `bd-2.1` → `bd`), for a
/// tracker `init --import` starts without `--prefix`: none when the export has no record, and
/// `VALIDATION` asking for `--prefix` when its records disagree, never one of theirs guessed at.
pub(crate) fn records_prefix(path: &OsStr) -> Result<Option<String>, Failure> {
    let content = read_utf8(path, path)?;
    let records = read_records(&content, path.to_str().unwrap_or_default())?;
    shared_prefix(&records)
}

/// The prefix every one of `records` is keyed under, as [`records_prefix`] reads it: what a dry
/// run asks of records it could read, whose other problems it lists itself. Beads keys an issue
/// `<prefix>-<hash>`, and some `<prefix>-<slug>-<hash>` (`syrup-rail-0u9` beside
/// `syrup-rail-simplification-audit-x1`), so the prefix is the leading `-`-separated segments every
/// id shares before its last one, never each id's text before its last `-`. Records that share
/// none (`ap-1` and `zz-1`) are `VALIDATION`, asking for `--prefix`.
pub(crate) fn shared_prefix(records: &[Value]) -> Result<Option<String>, Failure> {
    let stems: Vec<&str> = (records.iter())
        .filter_map(|record| record.get("id").and_then(Value::as_str))
        .filter_map(|id| id.rsplit_once('-').map(|(stem, _)| stem))
        .collect();
    let Some((first, rest)) = stems.split_first() else {
        return Ok(None);
    };
    let mut shared: Vec<&str> = first.split('-').collect();
    for stem in rest {
        let common = (shared.iter().zip(stem.split('-')))
            .take_while(|(a, b)| **a == *b)
            .count();
        shared.truncate(common);
    }
    let prefix = shared.join("-");
    // `-1` is keyed under no prefix, nor are records that share none: an empty one would be the
    // tracker's, its keys `-xyz`
    if prefix.is_empty() {
        let mut named: Vec<&str> = stems
            .iter()
            .map(|stem| stem.split('-').next().unwrap_or(""))
            .collect();
        named.sort_unstable();
        named.dedup();
        let named: Vec<String> = named
            .iter()
            .map(|n| {
                if n.is_empty() {
                    "''".to_owned()
                } else {
                    (*n).to_owned()
                }
            })
            .collect();
        return Err(Failure::new(
            Code::Validation,
            format!(
                "the records to import share no prefix (they begin {}): give --prefix",
                named.join(", ")
            ),
        ));
    }
    Ok(Some(prefix))
}

pub(crate) fn import_records(tx: &mut Tx<'_>, path: &OsStr) -> Result<Imported, Failure> {
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
    let mut taken_back = 0;
    for step in plan.steps {
        match step {
            Step::DepRemove { issue, data } => {
                tx.emit(OpName::DepRemove, Some(&issue.to_string()), data, None)?;
                taken_back += 1;
            }
            Step::Create(data) => {
                created.push(tx.emit(OpName::Create, None, data, None)?.to_string());
            }
            Step::DepAdd { issue, to, data } => {
                // a create the plan names that it does not hold is the planner's error, reported:
                // an empty id would be taken for the event's own
                let made = |n: usize| {
                    created.get(n).cloned().ok_or_else(|| {
                        Failure::new(
                            Code::Validation,
                            format!("the import's plan names create {n}, which it does not hold"),
                        )
                    })
                };
                let to = match to {
                    Target::Issue(uid) => uid.to_string(),
                    Target::Created(n) => made(n)?,
                };
                let mut payload = Object::new();
                payload.insert("to".into(), Value::from(to));
                payload.extend(data);
                let issue = made(issue)?;
                tx.emit(OpName::DepAdd, Some(&issue), payload, None)?;
            }
        }
    }
    Ok(Imported {
        issues: plan.imported,
        taken_back,
    })
}

/// The tracker `ws` keeps here, as its journal replays: read, and nothing written, where a view
/// keeps the projection it makes. Empty where there is no store.
pub(crate) fn read_only(ws: &Workspace) -> Result<State, Failure> {
    Ok(replay(load_journal(ws)?).map_err(ViewError::from)?)
}

/// What importing the export at `path` into `state`, `ws`'s tracker, would do: nothing imported, no
/// remote asked. `notes` say what the plan does not know, and come last.
pub(crate) fn answer_dry_run(
    ws: &Workspace,
    state: &State,
    path: &OsStr,
    notes: Vec<String>,
    refused_first: Option<String>,
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
    match (planned, refused_first) {
        (Ok((would, plan)), None) => {
            let retracted = (plan.steps.iter())
                .filter(|step| matches!(step, Step::DepRemove { .. }))
                .count();
            result.insert("importable".into(), Value::Bool(true));
            importable(&mut result, &mut lines, &would, retracted, &shown);
        }
        (planned, first) => {
            let mut found: Vec<String> = (problems(state, &content, &shown).iter())
                .map(ToString::to_string)
                .collect();
            if let (true, Err(refused)) = (found.is_empty(), &planned) {
                found.push(refused.to_string());
            }
            // what refuses the import before its records are read (`init`'s prefix) comes first
            if let Some(first) = first {
                found.insert(0, first);
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
