//! Bringing a Beads JSONL export in (Python's `cmd_import` and `import_records`): ids, timestamps, relationships and
//! unknown fields kept; one invalid record fails the whole import, before anything is written.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::import::{Step, Target, plan_import, read_records};
use beadroll_core::json::{Object, Value};
use beadroll_sync::{Tx, writing};

use super::{Context, Failure, Outcome, need_tracker, read_utf8};

/// `bead import PATH`: the export's records brought into this tracker, published at once.
pub(crate) fn import(ctx: &Context<'_>, path: &OsStr) -> Result<Outcome, Failure> {
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
    let plan = plan_import(tx.state()?, &records)?;
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
