//! `timing --decode N`: what reading a store's projection of N events costs in this process
//! (roll-lqjj.35, X16): its state part decoded alone, as a command that only reads decodes it
//! (`decode_state`), and the whole projection, as a write decodes it with its index
//! (`Projection::decode`), the fastest of [`RUNS`] runs each; the heap a decoded state holds, as
//! xtask's allocator counts it; and how many bytes the state part, the index and the texts take.
//! Replay and the files around a projection are not measured: `timing --read N` times the commands
//! themselves. Evidence, never a gate.

use std::time::Instant;

use beadroll_core::projection::{Projection, decode_state, project};

use super::heap;
use super::history::{Shape, history};
use super::memory;
use super::world::profile;
use crate::workspace::{self, Failure, json_string};

/// How many times each part is decoded; the fastest run counts.
const RUNS: usize = 7;

/// The fastest of [`RUNS`] runs of `run`, in seconds.
fn fastest<T>(mut run: impl FnMut() -> Result<T, Failure>) -> Result<f64, Failure> {
    let mut best = f64::INFINITY;
    for _ in 0..RUNS {
        let clock = Instant::now();
        let made = run()?;
        best = best.min(clock.elapsed().as_secs_f64());
        drop(made);
    }
    Ok(best)
}

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(count: usize, shape: Shape, label: Option<&str>) -> Result<String, Failure> {
    let dir = workspace::output_dir("timing")?.join(format!("decode-{}-{count}", shape.name()));
    memory::write_files(&dir, &history(count, shape))?;
    let journal = memory::load(&dir)?;
    // An ownership-heavy journal is a hundred thousand files: nothing to leave lying about.
    std::fs::remove_dir_all(&dir).map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    let projection =
        project(journal).map_err(|error| Failure::Failed(format!("replay failed: {error}")))?;
    let issues = projection.state().issues().count();
    let (encoded, _) = projection.encode(0);
    drop(projection);
    let unreadable = |error| Failure::Failed(format!("the projection does not read back: {error}"));
    let state_seconds = fastest(|| decode_state(&encoded.state).map_err(unreadable))?;
    let projection_seconds = fastest(|| Projection::decode(&encoded).map_err(unreadable))?;
    let before = heap::held();
    let state = decode_state(&encoded.state).map_err(unreadable)?;
    let state_heap = heap::held().saturating_sub(before);
    drop(state);
    let label = label.map_or_else(
        || format!("decode of {count} {}", shape.events()),
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"shape\": {}, \"events\": {count}, \"issues\": {issues}, \
         \"profile\": {}, \"runs\": {RUNS}, \"state_seconds\": {state_seconds:.4}, \
         \"projection_seconds\": {projection_seconds:.4}, \"state_heap_bytes\": {state_heap}, \
         \"state_bytes\": {}, \"index_bytes\": {}, \"texts_bytes\": {}, ",
        json_string(&label),
        json_string(shape.name()),
        json_string(profile()),
        encoded.state.len(),
        encoded.index.len(),
        encoded.texts.len(),
    ))
}
