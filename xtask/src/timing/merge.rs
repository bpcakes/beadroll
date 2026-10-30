//! `timing --merge N`: how long a `bead sync` takes that merges the events another machine wrote
//! after everything a store of N events holds (`--batch M` of them, ten by default), against a
//! local bare remote: with the store's projection in place (`seconds`), where the merged events are
//! applied after it, and removed before each sync (`cold_seconds`), where the journal is replayed,
//! as every sync did before roll-lqjj.2. With `--concurrent` the store writes a comment of its own
//! before each merge, so that the events merged were written concurrently with its own, the one
//! before among them, which the merge before replayed into the projection: the projection then
//! declines them and the journal is replayed (roll-lqjj.8; events concurrent only with writes left
//! after the projection would be extended, roll-lqjj.36), and the sync also publishes the store's
//! write. Evidence, never a gate.

use super::history::{ROTATE_BYTES, Shape, history};
use super::world::{RUNS, World, bead, profile, write_store};
use crate::workspace::{self, Failure, json_string};

/// Whether the timing history makes concurrent local writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MergeHistory {
    /// Sequential.
    Sequential,
    /// Concurrent.
    Concurrent,
}

/// How many events each merge brings when `--batch` does not say.
pub(crate) const BATCH: usize = 10;

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(
    count: usize,
    shape: Shape,
    batch: usize,
    concurrent: MergeHistory,
    label: Option<&str>,
) -> Result<String, Failure> {
    let concurrent = matches!(concurrent, MergeHistory::Concurrent);
    let bead = bead()?;
    let dir = workspace::output_dir("timing")?.join(format!("merge-{}-{count}", shape.name()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    }
    let files = history(count, shape);
    let here = World::new(&dir, bead, ROTATE_BYTES)?;
    let layout = write_store(&here.store(), &files, ROTATE_BYTES)?;
    drop(files);
    here.bead(&["sync"])?;
    let other = here.another(&dir.join("other"))?;
    other.bead(&["sync"])?;
    // the other machine writes after everything published, publishes, and this one merges
    let mut best = [f64::INFINITY; 2];
    for (cold, best) in [false, true].into_iter().zip(&mut best) {
        for run in 0..RUNS {
            for n in 0..batch {
                other.bead(&[
                    "comments",
                    "add",
                    "ex-1",
                    &format!("{run}.{n}"),
                    "--offline",
                ])?;
            }
            other.bead(&["sync"])?;
            if concurrent {
                here.write(&format!("here {run}"))?;
            }
            if cold {
                here.forget("projection")?;
            }
            *best = best.min(here.timed(&["sync"])?);
        }
    }
    let [warm, cold] = best;
    std::fs::remove_dir_all(&dir).map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    let label = label.map_or_else(
        || {
            let how = if concurrent {
                "concurrent merges"
            } else {
                "merges"
            };
            format!("{how} of {batch} into {count} {}", shape.events())
        },
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"shape\": {}, \"events\": {count}, \"batch\": {batch}, \
         \"concurrent\": {concurrent}, \"segments\": {}, \"attempt_files\": {}, \
         \"journal_bytes\": {}, \"profile\": {}, \"runs\": {RUNS}, \"seconds\": {warm:.4}, \
         \"cold_seconds\": {cold:.4}, ",
        json_string(&label),
        json_string(shape.name()),
        layout.segments,
        layout.attempts,
        layout.bytes,
        json_string(profile()),
    ))
}
