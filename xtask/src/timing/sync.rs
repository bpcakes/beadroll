//! `timing --sync N`: how long a publishing `bead sync` takes on a store of N events, against a
//! local bare remote. The store holds the history `timing --replay` generates as a store holds it:
//! each machine's journal in segments rotated as bead rotates them (at `--rotate BYTES`, bead's
//! default otherwise), and its attempt files. It is published once, then synced after each of a few
//! local writes: with the store's memo of its journal files and its projection in place
//! (`seconds`); with the projection removed before each sync (`unprojected_seconds`), as bead
//! synced before roll-lqjj.2; and with both removed (`cold_seconds`), as before roll-lqjj.1, every
//! file hashed and read and the journal replayed. Before those, the best of a few writes published
//! at once (`online_seconds`, `bead comments add` with the remote there), which read the store's
//! state once for the write and once for the sync's merge (roll-lqjj.29). Evidence, never a gate.

use super::history::{Shape, history};
use super::world::{RUNS, World, bead, profile, write_store};
use crate::workspace::{self, Failure, json_string};

pub(crate) use super::history::ROTATE_BYTES;

/// The store's memo of its journal files (roll-lqjj.1).
const MEMO: &str = "sync.git/journal-memo";

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(
    count: usize,
    shape: Shape,
    rotate: usize,
    label: Option<&str>,
) -> Result<String, Failure> {
    let bead = bead()?;
    let dir = workspace::output_dir("timing")?.join(format!("sync-{}-{count}", shape.name()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    }
    let files = history(count, shape);
    let world = World::new(&dir, bead, rotate)?;
    let layout = write_store(&world.store(), &files, rotate)?;
    drop(files);
    let first = world.timed(&["sync"])?;
    // a write published at once, as an agent's is
    let mut online = f64::INFINITY;
    for run in 0..RUNS {
        let text = format!("online {run}");
        online = online.min(world.timed(&["comments", "add", "ex-1", &text])?);
    }
    // the best sync with the store's memo and projection in place, without its projection, and
    // without either
    let mut best = [f64::INFINITY; 3];
    for (forgotten, best) in [&[][..], &["projection"], &["projection", MEMO]]
        .into_iter()
        .zip(&mut best)
    {
        for run in 0..RUNS {
            world.write(&format!("write {run}"))?;
            for name in forgotten {
                world.forget(name)?;
            }
            *best = best.min(world.timed(&["sync"])?);
        }
    }
    let [warm, unprojected, cold] = best;
    // An ownership-heavy store is a hundred thousand files: nothing to leave lying about.
    std::fs::remove_dir_all(&dir).map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    let label = label.map_or_else(
        || format!("sync of {count} {}", shape.events()),
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"shape\": {}, \"events\": {count}, \"rotate_bytes\": {rotate}, \
         \"segments\": {}, \"attempt_files\": {}, \"journal_bytes\": {}, \"profile\": {}, \
         \"runs\": {RUNS}, \"first_seconds\": {first:.4}, \"online_seconds\": {online:.4}, \
         \"seconds\": {warm:.4}, \"unprojected_seconds\": {unprojected:.4}, \
         \"cold_seconds\": {cold:.4}, ",
        json_string(&label),
        json_string(shape.name()),
        layout.segments,
        layout.attempts,
        layout.bytes,
        json_string(profile()),
    ))
}
