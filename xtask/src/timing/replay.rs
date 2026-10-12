//! `timing --replay N`: how long beadroll-core takes to replay N events (a parsed journal to its
//! State), against the budgets of Appendix A of `docs/RUST-PORT.md` (10,000 events in 20 ms,
//! 200,000 in 0.5 s and 150 MB, release build), and the peak memory of a process that loads that
//! journal from disk and replays it. Appendix A also holds what Python's replay took, measured
//! beside it until the cutover removed the Python implementation. Evidence, never a gate.

use std::ffi::OsStr;
use std::time::Instant;

use beadroll_core::replay::replay;

use super::history::{Shape, history};
use super::memory;
use crate::workspace::{self, Failure, json_string};

/// How many times the journal is replayed; the fastest run counts.
const RUNS: usize = 7;

/// A peak in bytes, or `null`.
fn bytes(peak: Option<u64>) -> String {
    peak.map_or_else(|| "null".to_owned(), |peak| peak.to_string())
}

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(count: usize, shape: Shape, label: Option<&str>) -> Result<String, Failure> {
    let dir = workspace::output_dir("timing")?.join(format!("replay-{}-{count}", shape.name()));
    memory::write_files(&dir, &history(count, shape))?;
    let parsed = memory::load(&dir)?;
    let events = parsed.events().len();
    let attempted = events.saturating_sub(parsed.order().values().map(Vec::len).sum());
    let mut best = f64::INFINITY;
    let mut issues = 0;
    for _ in 0..RUNS {
        let trial = parsed.clone();
        let clock = Instant::now();
        let state =
            replay(trial).map_err(|error| Failure::Failed(format!("replay failed: {error}")))?;
        best = best.min(clock.elapsed().as_secs_f64());
        issues = state.issues().count();
    }
    drop(parsed);
    let exe = std::env::current_exe()
        .map_err(|error| Failure::Failed(format!("cannot find xtask's own executable: {error}")))?;
    let load = [OsStr::new("timing"), OsStr::new("--load"), dir.as_os_str()];
    let peak = memory::peaks(&exe, &load)?;
    let no_replay = [load[0], load[1], load[2], OsStr::new("--no-replay")];
    let journal_peak = memory::peaks(&exe, &no_replay)?;
    // An ownership-heavy journal is a hundred thousand files: nothing to leave lying about.
    std::fs::remove_dir_all(&dir).map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let label = label.map_or_else(
        || match shape {
            Shape::Plain => format!("replay of {count} events"),
            Shape::Ownership => format!("replay of {count} ownership-heavy events"),
        },
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"shape\": {}, \"events\": {events}, \"attempted\": {attempted}, \
         \"issues\": {issues}, \"profile\": {}, \"runs\": {RUNS}, \"seconds\": {best:.4}, \
         \"peak_bytes\": {}, \"journal_peak_bytes\": {}, \"heap_peak_bytes\": {}, \
         \"journal_heap_peak_bytes\": {}, ",
        json_string(&label),
        json_string(shape.name()),
        json_string(profile),
        bytes(peak.resident),
        bytes(journal_peak.resident),
        bytes(peak.heap),
        bytes(journal_peak.heap),
    ))
}
