//! `cargo xtask`: the workspace's own tooling (`docs/RUST-PORT.md` §4).
//!
//! - `check` runs the workspace's baseline: its tests, and with options the slow and real ones.
//!   `./check.sh` is a thin wrapper around it.
//! - `timing` runs a command and records how long it took, as evidence.
//!
//! It is a developer tool, so it prints; the product crates do not (§2.2).
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

/// The system allocator, counted: `timing --load` reports the most its process held on its heap.
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
  timing --merge N [--batch M] [--ownership|--churn|--text] [--label NAME] [--out FILE]
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
      another machine wrote after everything the store holds, with the projection and without it.
      Timings are evidence, never a gate (docs/RUST-PORT.md §4.7).
";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
    let rest: Vec<_> = args.collect();
    let outcome = match command.as_ref().and_then(|c| c.to_str()) {
        Some("check") => check::main(rest),
        Some("timing") => timing::main(rest),
        Some("help" | "-h" | "--help") => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => Err(workspace::Failure::Usage(
            "expected a command: check or timing".into(),
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
