//! `timing --replay N`: how long beadroll-core takes to replay N events (a parsed journal to its
//! State), against the budgets of Appendix A of `docs/RUST-PORT.md` (10,000 events in 20 ms,
//! 200,000 in 0.5 s and 150 MB, release build), and the peak memory of a process that loads that
//! journal from disk and replays it; Python's replay of the same journal is measured beside it.
//! Evidence, never a gate.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use beadroll_core::replay::replay;

use super::history::{Shape, history};
use super::memory;
use crate::workspace::{self, Failure, json_string};

/// How many times each side replays; the fastest run counts.
const RUNS: usize = 7;

/// Python loads the journal in a directory as `load_journal` does; then, given a count of 0, it
/// replays it once (for its peak memory), or else that many times, each from a fresh copy, and
/// prints the fastest. Its replay of 200,000 events takes seconds, so it gets three runs.
const SCRIPT: &str = "\
import copy, sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import beadroll as bead
folder, runs = Path(sys.argv[2]), int(sys.argv[3])
journal = bead.Journal()
machines = sorted(p for p in folder.iterdir() if p.is_dir())
for machine in machines:
    for path in sorted(machine.glob('*.jsonl')):
        for event in bead.read_segment(path.read_bytes(), str(path), journal.invalid):
            journal.add(event, machine.name, str(path))
for machine in machines:
    for path in sorted((machine / 'claims').glob('*.json')):
        for event in bead.read_attempt(path, journal.invalid):
            journal.add(event, machine.name, str(path), sequenced=False)
journal.settle()
if runs == 0:
    st = bead.replay(journal)
    print(len(journal.events), len(st.issues))
else:
    best = None
    for _ in range(runs):
        trial = copy.deepcopy(journal)
        start = time.perf_counter()
        bead.replay(trial)
        took = time.perf_counter() - start
        best = took if best is None else min(best, took)
    print(best)
";

/// Python's fastest replay of the journal in `dir`, in seconds.
fn python(dir: &Path) -> Result<f64, Failure> {
    let output = Command::new(workspace::python())
        .args(["-c", SCRIPT])
        .arg(workspace::root())
        .arg(dir)
        .arg(RUNS.min(3).to_string())
        .stdin(Stdio::null())
        .output()
        .map_err(|error| Failure::Failed(format!("cannot run python: {error}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().parse().map_err(|_| {
        Failure::Failed(format!(
            "python answered {text:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    })
}

/// The peak of a Python process that loads the journal in `dir` and replays it once, in bytes.
fn python_peak(dir: &Path) -> Result<Option<u64>, Failure> {
    let python = which(&workspace::python());
    let root = workspace::root();
    let args = [
        OsStr::new("-c"),
        OsStr::new(SCRIPT),
        root.as_os_str(),
        dir.as_os_str(),
        OsStr::new("0"),
    ];
    memory::peak(&python, &args)
}

/// `name` as the shell would find it on PATH, or `name` itself.
fn which(name: &OsStr) -> std::path::PathBuf {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| name.into())
}

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
    let peak = memory::peak(&exe, &load)?;
    let no_replay = [load[0], load[1], load[2], OsStr::new("--no-replay")];
    let journal_peak = memory::peak(&exe, &no_replay)?;
    let python_seconds = python(&dir)?;
    let python_peak = python_peak(&dir)?;
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
         \"peak_bytes\": {}, \"journal_peak_bytes\": {}, \"python_seconds\": {python_seconds:.4}, \
         \"python_peak_bytes\": {}, ",
        json_string(&label),
        json_string(shape.name()),
        json_string(profile),
        bytes(peak),
        bytes(journal_peak),
        bytes(python_peak),
    ))
}
