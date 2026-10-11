# beadroll crate guide

## Purpose

`crates/beadroll` is the `bead` binary (`docs/RUST-PORT.md` §2.1): `commands` maps typed requests to
typed outcomes, `cli` parses arguments, renders results and owns the output streams and exit codes.
Slices 2a and 2b (`roll-k2wy.3`, `roll-k2wy.4`) port every command but the coordinated ones, which
Slice 2c (`roll-k2wy.5`) adds: `claim`, `release`, `takeover` and `update --claim`/`--status
in_progress`, with their receipts. Slice 2d (`roll-k2wy.6`) added `sys`, one of the two audited
unsafe spots, and the Python suite passed against it whole. Since the cutover (`roll-k2wy.16`) it
is bead: the Python implementation it replaced is at `python-0.1.0`. Its integration tests are the
black-box tests ported from the Python suite, the golden outputs captured from Python, and
`parity`, what the mixed-fleet tests held it to once Python was gone.

## Key entrypoints

- `src/main.rs`: the executable; it calls `beadroll::run`.
- `src/sys.rs`: what bead needs of the operating system around `main` (§2.3f), one of the two
  audited unsafe spots: a constructor that records whether standard input was open at exec, before
  the runtime reopens it on `/dev/null`; SIGXFSZ ignored, as CPython ignores it; and the descriptors
  bead was started with kept from the processes it starts, as Python's `close_fds=True` keeps them.
- `src/cli.rs`: the flow of one run (`--json`, `--actor` and `--` read first, settings, the
  parse, the workspace, the command, publishing what it wrote, the unpublished count, rendering,
  errors). Its parts:
  - `src/cli/spec.rs`: every command's arguments, as Python's `build_parser` declares them; the
    one table the parser, the help pages and (from 2c) the receipt signature read.
  - `src/cli/parse.rs`, `parse/classify.rs`, `parse/values.rs`: CPython 3.14's argparse, ported
    (§2.3h): what is an option, how many values each takes, the namespace, the errors.
  - `src/cli/help.rs`, `src/cli/wrap.rs`: argparse's `HelpFormatter` and `textwrap.wrap`.
  - `src/cli/free.rs`: `help`, `prime` and `onboard`, which work outside a repository;
    `src/cli/texts.rs`: the overview, `PRIME` and `ONBOARD`, verbatim.
  - `src/cli/output.rs`: the EPIPE-safe writes and `visible`; `src/cli/tempdir.rs`: Python's
    `gettempdir`.
- `src/commands.rs`: `Failure` (with the raw bytes of a message that holds some not UTF-8),
  `Outcome` and the helpers commands share; `src/commands/args.rs`: command-line text as Python's
  `surrogateescape` strings behave (`resolve_arg`, `field_args`, `Payload`, `encodable`);
  `src/commands/*.rs`: one command or a family each (`transition.rs` is close, reopen and delete;
  `queries.rs` blocked, search and lint; `history.rs` log and machines; `ownership.rs` claim,
  takeover and release, the claim step `update` shares, and `command_signature`, the receipt).
- `tests/cli/main.rs`: the one test binary's root; `sandbox()` hands test support the binary Cargo
  built (`env!("CARGO_BIN_EXE_bead")`), the only place that default can come from.
- `tests/cli/commands/*.rs`: `tests/test_commands.py`, ported; `tests/cli/golden.rs` and
  `golden/normal.rs`: the golden scenarios and their normalisation, against the files in
  `tests/golden/`, captured from Python's `./bead` before the cutover.
- `tests/cli/parity.rs` and `parity/*.rs`: what the mixed-fleet tests held the Rust bead to, on
  the Rust bead alone: machines converge and export alike, the published tree is format 1's, sync
  settles pending attempts, a retry recognises its receipt (`parity/retries.rs`), and the answers
  that were Python's own, pinned as captured from it: the receipts' bytes (`parity/receipts.rs`),
  the unpublished counts (`parity/pending.rs`), and what bead says to arguments, actors and paths
  that are not UTF-8 (`parity/inputs.rs`, `parity/actors.rs`).
- `tests/cli/protocol.rs`, `hardening.rs`, `storage.rs`, `compat.rs`, `replay.rs`: the black-box
  tests of `test_protocol`, `test_hardening`, `test_storage`, `test_compat` (its real-export test
  `#[ignore]`d, run by `cargo xtask check --real`) and `test_replay`, ported; `hardening/gate.rs`:
  the `pause-until` handshake (§4.5) a test holds bead at. `tests/cli/sweep.rs`: `test_sweep`, the
  ownership protocol enumerated, its boundaries reached through failpoints (`fault_env`); the
  twelve-schedule slice runs in the fast suite, and `sweep/sweep_failure_reporting.rs`
  (`test_check.SweepFailureReporting`) checks that a failed schedule fails it.
  `tests/cli/stress.rs`: `test_stress`'s two randomised models, ignored by default; they draw
  from `stress/python_random.rs`, CPython's `random.Random`, so a seed makes Python's choices.
- `main.rs`'s `rust_only`: the machine of a test that injects a fault through a failpoint, where
  the Python test patched bead in process, reads its store in the test's process, or checks one of
  the Rust bead's own departures from Python (§5 row 26). Those are this build's, so it runs this
  build's binary whatever `BEAD_UNDER_TEST` says.

## Edit here for X

- A test of a command: `tests/cli/<module>/<class>.rs` beside the tests of its subject. The ported
  tests are named by the rule of `docs/RUST-PORT.md` §4.2, `test_commands.HelpAndPrime.test_x`
  becoming `commands::help_and_prime::x`, which keeps `docs/FINDINGS.md`'s references valid; a
  test of a finding names the finding.
- A command, its output or its exit status: `src/commands` and `src/cli`.
- An argument or a help text: `src/cli/spec.rs`, as Python's `build_parser` declared it. The help
  pages are goldens (`tests/golden/help.txt`): a deliberate change edits the golden by hand in the
  same change. An argument of a coordinated command is part of its receipt (`Namespace::asked`):
  `parity::receipts`, Python's bytes, must stay green.
- A golden scenario: `tests/cli/golden.rs`. Its file in `tests/golden/` is Python's, captured
  before the cutover (`python-0.1.0` has the capture, `cargo xtask golden`); a change to what bead
  prints that is meant edits it by hand, in the same change, which says so.

## Invariants

- Only `cli` reads the process environment, the working directory and the clock; it passes values
  down (§2.2).
- No `print!`, `println!` or `eprintln!`: every write goes through `cli/output.rs`, where a closed
  pipe or a hung-up terminal is a quiet exit 0 and a failed write to stderr keeps the status.
- Output keeps Python's dialects: `--json` results spaced with raw UTF-8, errors and `--version`
  ASCII-escaped, human lines with control characters escaped.
- The command line is argparse's: what parses, what fails and with which text; help pages equal
  Python's at every width. Arguments stay `OsString` until a command reads them, and fail where
  Python's `surrogateescape` strings would.
- The goldens and the answers `parity` pins are Python's, captured before the cutover; neither is
  ever regenerated to make the Rust bead pass. Only a change of behaviour that is meant, and
  recorded as such, changes one, by hand.
- Test roots start with `#![cfg(test)]`; helpers shared between test modules are `pub(crate)`.
- Every test that does not need this build's own binary (`rust_only`) runs whatever bead
  `BEAD_UNDER_TEST` names, so the suite can be pointed at an installed bead, or at
  `python-0.1.0`'s launcher during the rollback window. A `rust_only` test that replaces a Python
  test was reviewed against it: each failpoint fires where the Python patch did, as often as it
  did.

## Common commands

- `cargo nextest run -p beadroll`: the black-box tests against the binary this package builds
- `BEAD_UNDER_TEST=/abs/path/bead cargo nextest run -p beadroll`: the same against another bead
- `cargo nextest run -p beadroll -E 'test(/^golden::/) | test(/^parity::/)'`: the bead against
  what Python answered
- `BEAD_SWEEP=full cargo nextest run -p beadroll -E 'test(=sweep::sweep::sweep)'`: all 660
  schedules of the sweep, in parallel (about 11 minutes on 16 cores); `BEAD_SWEEP_GC=1` and
  `BEAD_SWEEP_CONCURRENT=1` select its variants
- `cargo nextest run -p beadroll --run-ignored only -E 'test(/^stress::/)'`: the stress models,
  8 to 20 minutes each
- `cargo install --locked --force --path crates/beadroll --root ~/.local`, from the checkout's
  root: install the binary as `~/.local/bin/bead` (`README.md`, *Install*)
