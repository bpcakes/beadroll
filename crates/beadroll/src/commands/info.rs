//! `bead info`: where the tracker lives, who you are, and how its journal stands.

use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::json::{Object, Value};
use beadroll_core::projection::Projection;
use beadroll_core::text::{printed_text, str_of};
use beadroll_store::{
    Workspace, dir_entries, load_pending, projection, replica_id, segment_files,
    tracked_config_warning,
};

use super::{Context, Failure, Outcome};

/// The version `info` reports.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The tracker's identity and place, the remote, this machine and actor, and the journal's
/// numbers: issues, events, segments, the frontier, attempts awaiting the remote, integrity
/// problems and renamed keys; with any `sync.git` set aside as damaged.
pub(crate) fn info(ctx: &Context<'_>) -> Result<Outcome, Failure> {
    let ws = ctx.ws;
    let projection = projection(ws)?;
    let st = projection.state();
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
    // only when there is one: a tracked configuration the other checkouts do not see
    if let Some(warning) = tracked_config_warning(ws)? {
        result.insert("tracked_config_warning".into(), Value::from(warning));
    }
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
    let label = ctx.actor.label(replica.as_deref().unwrap_or("?"));
    result.insert("actor_label".into(), Value::from(label.shown()));
    numbers(ws, &projection, &mut result)?;
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
///
/// None of them reads the journal (roll-lqjj.31): the issues, the waiting events, the integrity
/// problems and the renamed keys are the store's projection's state; the events, each machine's
/// and the frontier its index (every event id, each machine's journal length, the heads); the
/// segments the files' listing; the attempts awaiting the remote this machine's pending files.
/// The journal is read only when the projection is not the journal's, to make it.
fn numbers(ws: &Workspace, projection: &Projection, result: &mut Object) -> Result<(), Failure> {
    let st = projection.state();
    result.insert("issues".into(), Value::from(st.issues().count() as i64));
    let events = projection.event_count();
    result.insert("events".into(), Value::from(events as i64));
    let mut by_machine = Object::new();
    for (replica, length, _) in projection.journals() {
        by_machine.insert(replica.to_owned(), Value::from(i64::from(length)));
    }
    result.insert("events_by_machine".into(), Value::from(by_machine));
    result.insert(
        "segments".into(),
        Value::from(segment_files(ws)?.len() as i64),
    );
    let frontier = (projection.heads().iter())
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
        .map(|row| Value::from(row.to_value()))
        .collect();
    result.insert("integrity".into(), Value::Array(integrity));
    let renames = st
        .renames()
        .iter()
        .map(|rename| Value::from(rename.to_value()))
        .collect();
    result.insert("renamed_keys".into(), Value::Array(renames));
    Ok(())
}
