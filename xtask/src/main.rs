//! `cargo xtask`: the workspace's own tooling (`docs/RUST-PORT.md` §4).
//!
//! - `oracle` runs the Python test suite against a `bead` binary, through `BEAD_UNDER_TEST`.
//! - `inventory` maps every Python test to the Rust test that ports it.
//! - `timing` runs a command and records how long it took, as evidence.
//! - `diff` checks beadroll-core against CPython: replay reports and JSON, case by case.
//! - `golden` captures the golden outputs from Python's launcher.
//! - `check` runs the workspace's baseline: its tests, and with options the slow and real ones.
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
mod diff;
mod golden;
#[cfg(test)]
mod hygiene;
mod inventory;
mod oracle;
mod timing;
mod workspace;

use std::process::ExitCode;

const USAGE: &str = "\
usage: cargo xtask <command>

  check [--stress] [--real]
      Run the workspace's baseline with a scratch BEAD_HOME, one line per step, and fail when any
      step fails: the tests through nextest (BEAD_UNDER_TEST and BEAD_SWEEP reach them as set),
      the doctests, and the Python tests of the infra/ drivers. --stress adds the stress models;
      --real adds the real-export compatibility test and infra/roundtrip.py over this machine's
      Beads exports, against BEAD_UNDER_TEST or else the bead Cargo builds. Each step's whole
      output is kept in target/check/.

  oracle [--bead PATH] [--report] [--stress] [--jobs N] [MODULE...]
      Run the Python test suite against a bead binary through BEAD_UNDER_TEST, without the tests
      listed in xtask/oracle-skip.txt, and report each module. The binary is the bead that Cargo
      builds first (target/debug/bead by default) unless --bead names another; `--bead ./bead` runs
      Python's own launcher, against which the suite must pass completely. The modules are
      check.sh's fast ones, --stress adds test_stress and test_sweep, or name them. --report exits
      0 when tests fail, to show every module's count; --jobs runs that many modules at once
      (default 1).

  inventory [--require-complete]
      List every Python test with the Rust tests that port it (docs/RUST-PORT.md §4.2), from
      xtask/inventory.txt and the tests both suites have. --require-complete fails while any
      Python test is unported.

  diff replay|json [--cases N] [--seed S] [--jobs N] [--fixtures FILE...] [--write-cases FILE]
      Check beadroll-core against CPython (docs/RUST-PORT.md §4.2): every case goes through a
      stdlib-only Python driver (xtask/python/) and the Rust core, and the answers must be the same
      text. replay replays journals: the fixtures in xtask/fixtures/replay/ (or --fixtures), then N
      generated histories (default 1000) from seed S (default 1). json parses and writes: its
      corpus (or --fixtures), then N generated values and journal lines. A differing case is kept
      in target/diff/ to rerun alone with --fixtures; --write-cases keeps every case.

  golden
      Capture the golden outputs (crates/beadroll/tests/golden/: every help page, the human output
      of the reading commands over one scripted tracker, info's keys) from Python's ./bead, by
      running the golden tests in capture mode against it (docs/RUST-PORT.md §4.2). Goldens come
      from Python only; the same tests compare any other bead with them.

  timing [--label NAME] [--out FILE] -- COMMAND [ARG...]
  timing --replay N [--label NAME] [--out FILE]
      Run COMMAND and append what it took, one JSON line, to FILE (default
      target/timing.jsonl); or replay a parsed journal of N valid events in-process, the best of
      seven runs, with Python's replay of the same journal beside it (run it from a release
      build: cargo run --release --package xtask -- timing --replay 10000). Timings are
      evidence, never a gate (docs/RUST-PORT.md §4.7).
";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
    let rest: Vec<_> = args.collect();
    let outcome = match command.as_ref().and_then(|c| c.to_str()) {
        Some("oracle") => oracle::main(rest),
        Some("inventory") => inventory::main(rest),
        Some("timing") => timing::main(rest),
        Some("diff") => diff::main(rest),
        Some("golden") => golden::main(rest),
        Some("check") => check::main(rest),
        Some("help" | "-h" | "--help") => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => Err(workspace::Failure::Usage(
            "expected a command: check, oracle, inventory, timing, diff or golden".into(),
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
