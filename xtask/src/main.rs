//! `cargo xtask`: the workspace's own tooling.
//!
//! - `check` runs the workspace's baseline: its tests, and with options the slow and real ones.
//!   `./check.sh` is a thin wrapper around it.
//! - `coverage` reports what the suites run of the product's code, with cargo-llvm-cov.
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
mod coverage;
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

  coverage [--stress] [--sweep]
      Run the suites under cargo-llvm-cov and report which lines of the product's code they run:
      an HTML report (target/coverage/html/index.html), target/coverage/lcov.info and
      target/coverage/summary.json, then the lines by crate, the ownership protocol's files with
      the lines no test reaches, and whether the black-box tests' bead processes are in the
      count. It runs the workspace's tests through nextest, as check does, the sweep's slice among
      them; --stress adds the stress models and --sweep all 660 schedules of the ownership sweep,
      the slow suites, which the nightly workflow runs. The doctests (the workspace has none) and
      the growth guards are left out, and BEAD_UNDER_TEST is ignored: the bead counted is the one
      the build makes. It needs cargo-llvm-cov and the toolchain's llvm-tools, which it asks you
      to install when they are missing (rust-toolchain.toml has neither). Fails when a suite
      fails, the report still made; when a profile could not be merged into the report, so that
      its counts are missing; or when the report does not count the bead processes. A bead
      killed by SIGKILL, or run under a resource limit, writes no profile. Each step's whole
      output is kept in target/coverage/logs/; nothing is installed or published.

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
  timing --dangling N [--label NAME] [--out FILE]
  timing --ready N [--label NAME] [--out FILE]
  timing --details N [--label NAME] [--out FILE]
  timing --renames N [--label NAME] [--out FILE]
  timing --decode N [--ownership|--churn|--text] [--label NAME] [--out FILE]
  timing --parse N [--ownership|--churn|--text] [--label NAME] [--out FILE]
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
      default, 262144), against a local bare remote: the first sync, the best of five writes
      published at once (bead comments add), then the best of five syncs after a local write
      each, with the store's memo of its journal files and its projection, without its
      projection, and without either. --read N times bead list, show, ready, log --limit 1,
      info and machines there, and a local write (bead comments add --offline), with the projection and without it, a write cycle: a write after a read, the bead show after it, and a write after a write, a bead claim and bead release of a new issue, each published at once, and a write after a read once the store has no remote (not --offline); --merge N, a bead sync that merges M events (ten by default)
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
      ids sort before every id held, N label and N dependency additions one at a time, N label
      additions to one issue in a single batch, and N closes one at a time after an event of an
      operation a newer bead writes, the fastest of three runs each; --dangling N, replay in this
      process of N creates whose imports name keys no issue has yet: none (the control), keys that
      never come, keys of issues created later, the first issue naming every other's key as they
      come one create at a time, and the last naming them once they all exist (its control), and
      a projection of that first issue taking those creates a batch at a time against the same
      creates waiting for nothing, the fastest of three runs each; --ready N,
      readiness judged in this process on N issues in a chain held by its root's prerequisite, the
      same chain free, a lattice whose rows share their ancestors, and that lattice beneath gates
      of its own for its first rows: for every issue at once (the analysis alone, and bead
      ready's answer with its sort), issue by issue, and by the complete blocker lists made issue
      by issue and all at once, the fastest of three runs each;
      --details N, every issue of N on a sparse graph without parents shown in full in this
      process, the issues that depend on each found once for all of them and for each alone, the
      reverse lookup alone both ways, and for one issue its detail and the lookup both ways as a
      control, the fastest of three runs each; --renames N, replay in this process of two machines
      each creating the same N keys, so that N copies are renamed, against the control of 2N
      creates renamed by nobody, and each again with the first machine's issues importing a
      dependency on a key that never comes, the fastest of three runs each; --decode N, the
      projection of N events decoded in this process: its state part alone, as a command that only
      reads decodes it, and whole with its index, as a write does, the fastest of seven runs each,
      the heap a decoded state holds, and the bytes of the state part, the index and the texts;
      and its encode after a comment, as a write-back between compactions of what every event had
      seen encodes it, copying what did not change since it was read back, and whole, every record
      written; at the worst point, an eighth more events taken in than a compaction left, the
      index's bytes, the decode, the heap held whole and the encode, copying and whole; and one
      comment past it the encode that compacts; --parse N, core's JSON reader in this process on
      meta.json with a published map of N entries, parsed whole as every command with a remote
      parses it for its unpublished count, its members put into an object once and twice (by key
      as written, then by key as read: the reader's own double build on its own), and on every
      line of a history of N events, the fastest of seven runs each.
      Timings are evidence, never a gate.

  bead ARG...
      Run bead with ARGs in this process, whose heap is counted, and say on standard error the
      most it held at once (heap peak: N) and what it still held when bead returned (heap held:
      N, a few kilobytes when bead freed all it made) once it is done: how timing --import counts
      bead's heap, and what xtask's tests/heap test.
";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
    let rest: Vec<_> = args.collect();
    let outcome = match command.as_ref().and_then(|c| c.to_str()) {
        Some("check") => check::main(rest),
        Some("coverage") => coverage::main(rest),
        Some("timing") => timing::main(rest),
        Some("bead") => return timing::memory::bead(rest),
        Some("help" | "-h" | "--help") => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => Err(workspace::Failure::Usage(
            "expected a command: check, coverage, timing or bead".into(),
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
