//! `cargo xtask`: the workspace's own tooling.
//!
//! - `check` runs the workspace's baseline: its tests, and with options the slow and real ones.
//!   `./check.sh` is a thin wrapper around it.
//! - `timing` runs a command and records how long it took, as evidence.
//!
//! It is a developer tool, so it prints; the product crates do not (`CONTRIBUTING.md`).
#![allow(
    clippy::print_stdout,
    reason = "a developer tool reports on its own terminal"
)]
#![allow(
    clippy::print_stderr,
    reason = "a developer tool reports on its own terminal"
)]

mod check;
#[cfg(test)]
mod ci;
#[cfg(test)]
mod hygiene;
mod timing;
mod workspace;

use std::process::ExitCode;

/// The system allocator, counted: `timing --load` and `bead` report the most their process held on
/// its heap.
#[global_allocator]
static HEAP: timing::heap::Counting = timing::heap::Counting;

const USAGE: &str = "\
usage: cargo xtask <command>

  check [--stress] [--real]
      Run the workspace's baseline with a scratch BEAD_HOME, one line per step, and fail when any
      step fails: the tests through nextest (BEAD_UNDER_TEST and BEAD_SWEEP reach them as set),
      the doctests, and the Python tests of the infra/ drivers. --stress adds the stress models;
      --real adds the real-export compatibility test and infra/roundtrip.py over this machine's
      Beads exports, against BEAD_UNDER_TEST or else the bead Cargo builds. Each step's whole
      output is kept in target/check/. ./check.sh runs this.

  timing [--label NAME] [--out FILE] -- COMMAND [ARG...]
  timing --replay N [--ownership|--churn|--text] [--label NAME] [--out FILE]
  timing --sync N [--ownership|--churn|--text] [--rotate BYTES] [--label NAME] [--out FILE]
  timing --read N [--ownership|--churn|--text] [--label NAME] [--out FILE]
  timing --merge N [--batch M] [--concurrent] [--ownership|--churn|--text] [--label NAME]
         [--out FILE]
  timing --contend M [--writes W] [--latency MS] [--label NAME] [--out FILE]
  timing --import N [--label NAME] [--out FILE]
  timing --plan N [--label NAME] [--out FILE]
  timing --extend N [--label NAME] [--out FILE]
  timing --ready N [--label NAME] [--out FILE]
  timing --load DIR [--no-replay]
      Run COMMAND and append what it took, one JSON line, to FILE (default
      target/timing.jsonl); or replay a parsed journal of N valid events in-process, the best of
      seven runs (run it from a release build: cargo run --release --package xtask -- timing
      --replay 10000). --ownership makes about half the events claims, takeovers and releases in
      attempt files; --churn keeps a thousand issues, every later event retitling one or putting
      a label on or off it; --text gives the plain history's operations texts the size of real
      trackers' (descriptions, notes and comments of a few hundred bytes to a few KB). The journal is written under target/timing/ while it is measured, and
      the record also holds the peak memory (as /usr/bin/time reports it) of a process that loads it
      from disk and replays it, and of one that only loads it: --load DIR is that process.
      --sync N times a publishing bead sync (BEAD_UNDER_TEST, else the bead Cargo builds in
      xtask's profile) on a scratch store of those N events, its journals rotated at BYTES (bead's
      default, 262144), against a local bare remote: the first sync, then the best of five after a
      local write each, with the store's memo of its journal files and its projection, without
      its projection, and without either. --read N times bead list, show and ready there, and a
      local write (bead comments add --offline), with the projection and without it; --merge N, a bead sync that merges M events (ten by default)
      another machine wrote after everything the store holds, with the projection and without it
      (--concurrent: the store writes too, so that the merge brings concurrent events);
      --contend M, M machines writing one tracker at once (W writes each, 25 by default), every
      write published, each fetch and push made MS milliseconds slower (300 by default); --import
      N, the time, peak resident set and heap peak of bead init --import of a generated export of
      N records, shaped as real trackers' are, offline and then the first sync, and published;
      --plan N, the core calls an import of N records makes in this process, each record
      depending on a key at the end of the batch: planning it, validating it for a dry run, and
      planning a re-import of one known issue whose N dependencies are all still unresolved, the
      fastest of five runs each; --extend N, events applied to a projection of N issues in this
      process, each with two labels and a tenth of them a parent of nine: N closes one event at a
      time as a bulk close applies them, the same closes in one batch, N closes one at a time whose
      ids sort before every id held, N label and N dependency additions one at a time, and N label
      additions to one issue in a single batch, the fastest of three runs each; --ready N,
      readiness judged in this process on N issues in a chain held by its root's prerequisite, the
      same chain free, and a lattice whose rows share their ancestors: for every issue at once
      (the analysis alone, and bead ready's answer with its sort), issue by issue, and by the
      complete blocker lists every reader built before, the fastest of three runs each. Timings
      are evidence, never a gate.

  bead ARG...
      Run bead with ARGs in this process, whose heap is counted, and say on standard error the
      most it held at once (heap peak: N) once it is done: how timing --import counts bead's heap.
";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
    let rest: Vec<_> = args.collect();
    let outcome = match command.as_ref().and_then(|c| c.to_str()) {
        Some("check") => check::main(rest),
        Some("timing") => timing::main(rest),
        Some("bead") => return timing::memory::bead(rest),
        Some("help" | "-h" | "--help") => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => Err(workspace::Failure::Usage(
            "expected a command: check, timing or bead".into(),
        )),
    };
    match outcome {
        Ok(code) => code,
        Err(workspace::Failure::Usage(message)) => {
            eprintln!("xtask: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(workspace::Failure::Failed(message)) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}
