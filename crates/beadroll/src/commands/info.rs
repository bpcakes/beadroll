//! `bead info`: where the tracker lives, who you are, and how its journal stands.

use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::journal::Journal;
use beadroll_core::json::{Object, Value};
use beadroll_core::state::{State, actor_label};
use beadroll_core::text::{printed_text, str_of};
use beadroll_store::{
    Workspace, dir_entries, load_journal, load_pending, replica_id, segment_files, view,
};

use super::{Context, Failure, Outcome};

/// The version `info` reports.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The tracker's identity and place, the remote, this machine and actor, and the journal's
/// numbers: issues, events, segments, the frontier, attempts awaiting the remote, integrity
/// problems and renamed keys; with any `sync.git` set aside as damaged.
pub(crate) fn info(ctx: &Context<'_>) -> Result<Outcome, Failure> {
    let ws = ctx.ws;
    let mut journal = load_journal(ws)?;
    let st = view(ws)?;
    let replica = replica_id(&ws.settings, false)?;
    let remote = match ws.remote(None) {
        Ok(Some(remote)) => Value::from(printed_text(remote.as_bytes())),
        Ok(None) => Value::Null,
        // info is where a damaged meta.json is diagnosed, not where it stops
        Err(error) => Value::from(format!("unknown ({}: {error})", error.code().as_str())),
    };
    let source = if ws.settings.var("BEAD_WORKSPACE").is_some() {
        "BEAD_WORKSPACE"
    } else if ws.config.get("workspace").is_some_and(Value::truthy) {
        "tracked .beadroll.json"
    } else if ws.origin.as_ref().is_some_and(|o| !o.is_empty()) {
        "origin url"
    } else {
        "local path"
    };
    let name = match st.name().filter(|n| n.truthy()) {
        Some(name) => name.clone(),
        None => ws
            .name
            .as_ref()
            .map_or(Value::Null, |n| Value::from(printed_text(n.as_bytes()))),
    };
    let mut result = Object::new();
    result.insert("version".into(), Value::from(VERSION));
    result.insert("workspace".into(), Value::from(ws.id.as_str()));
    result.insert("workspace_source".into(), Value::from(source));
    result.insert("name".into(), name);
    result.insert("prefix".into(), st.prefix().cloned().unwrap_or(Value::Null));
    result.insert(
        "path".into(),
        Value::from(printed_text(ws.dir.as_os_str().as_bytes())),
    );
    result.insert("remote".into(), remote);
    result.insert("ref".into(), Value::from(ws.ref_name()?));
    result.insert("machine".into(), Value::from(replica.clone()));
    result.insert("actor".into(), ctx.actor.value.clone());
    result.insert(
        "actor_label".into(),
        Value::from(actor_label(
            Some(&ctx.actor.value),
            replica.as_deref().unwrap_or("?"),
        )),
    );
    numbers(ws, &mut journal, &st, &mut result)?;
    let synced = ws.meta(false)?.get("last_sync").cloned();
    result.insert("last_sync".into(), synced.unwrap_or(Value::Null));
    // set aside by a rebuild; nothing in them is the only copy of anything, delete at leisure
    let damaged: Vec<Value> = dir_entries(&ws.dir)?
        .iter()
        .filter_map(|path| path.file_name())
        .filter(|name| name.as_bytes().starts_with(b"sync.git.damaged-"))
        .map(|name| Value::from(printed_text(name.as_bytes())))
        .collect();
    if !damaged.is_empty() {
        result.insert("damaged_sync_repos".into(), Value::Array(damaged));
    }
    let lines = result
        .iter()
        .map(|(key, value)| format!("{key}: {}", str_of(value)))
        .collect();
    Ok(Outcome::new(result, lines))
}

/// The journal's numbers, in `info`'s order: issues, events in all and by machine, segments, the
/// frontier, this machine's attempts awaiting the remote, events waiting for what they build on,
/// integrity problems and renamed keys.
fn numbers(
    ws: &Workspace,
    journal: &mut Journal,
    st: &State,
    result: &mut Object,
) -> Result<(), Failure> {
    result.insert("issues".into(), Value::from(st.issues().count() as i64));
    result.insert("events".into(), Value::from(journal.events().len() as i64));
    let mut machines: Vec<_> = journal.order().iter().collect();
    machines.sort_by(|a, b| a.0.cmp(b.0));
    let mut by_machine = Object::new();
    for (replica, ids) in machines {
        by_machine.insert(replica.clone(), Value::from(ids.len() as i64));
    }
    result.insert("events_by_machine".into(), Value::Object(by_machine));
    result.insert(
        "segments".into(),
        Value::from(segment_files(ws)?.len() as i64),
    );
    let frontier = journal
        .heads()
        .into_iter()
        .map(|id| Value::from(id.to_string()))
        .collect();
    result.insert("frontier".into(), Value::Array(frontier));
    let pending = load_pending(ws)?
        .iter()
        .flatten()
        .map(|event| event.get("id").cloned().unwrap_or(Value::Null))
        .collect();
    result.insert("pending_ownership".into(), Value::Array(pending));
    result.insert(
        "waiting_events".into(),
        Value::from(st.pending().len() as i64),
    );
    let integrity = st
        .integrity()
        .iter()
        .map(|row| Value::Object(row.to_value()))
        .collect();
    result.insert("integrity".into(), Value::Array(integrity));
    let renames = st
        .renames()
        .iter()
        .map(|rename| Value::Object(rename.to_value()))
        .collect();
    result.insert("renamed_keys".into(), Value::Array(renames));
    Ok(())
}
