# beadroll crate guide

## Purpose

`crates/beadroll` is the `bead` binary: `commands` maps typed requests to typed outcomes, `cli`
parses arguments, renders results and owns the output streams and exit codes.
Coordinated commands (`claim`, `release`, `takeover` and `update --claim`/`--status in_progress`)
use receipts. Integration tests cover commands, golden outputs, cross-version compatibility,
storage, protocol behavior and fault recovery.

## Key entrypoints

- `src/main.rs`: the executable; it calls `beadroll::run`.
- `src/sys.rs`: what bead needs of the operating system around `main`, one of the two
  audited unsafe spots: a constructor that records whether standard input was open at exec, before
  the runtime reopens it on `/dev/null`; SIGXFSZ ignored; and inherited descriptors kept from
  child processes.
- `src/cli.rs`: the flow of one run (`--json`, `--actor`, `--color` and `--` read first, settings,
  the parse, the workspace, the command, publishing what it wrote, the unpublished count,
  rendering, errors). Its parts:
  - `src/cli/spec.rs`, `spec/args.rs`: every command's arguments; the one table the parser, help
    pages and receipt signature read.
  - `src/cli/parse.rs`: clap's command line, built from the table, its matches turned into the
    namespace commands read and receipts sign (roll-dih8.23).
  - `src/cli/help.rs`: a command's help page as clap renders it; `src/cli/paint.rs`: whether a
    stream shows colour, and each part of a line escaped, then painted.
  - `src/cli/free.rs`: `help`, `prime` and `onboard`, which work outside a repository;
    `src/cli/texts.rs`: the overview, `PRIME` and the help pages' epilogs as paragraphs and rows,
    and `ONBOARD` verbatim; `src/cli/layout.rs`: laying those out for the reader's width.
  - `src/commands/line.rs`: a line of human output as parts that say what they are (`Tone`).
  - `src/cli/output.rs`: the EPIPE-safe writes and `visible`; `src/cli/tempdir.rs`: writable
    temporary-directory selection with environment overrides and fallbacks.
- `src/commands.rs`: `Failure` (with the raw bytes of a message that holds some not UTF-8),
  `Outcome` and the helpers commands share; `src/commands/args.rs`: command-line text with invalid
  UTF-8 preserved through `surrogateescape` (`resolve_arg`, `field_args`, `Payload`, `encodable`);
  `src/commands/*.rs`: one command or a family each (`transition.rs` is close, reopen and delete;
  `queries.rs` blocked, search and lint; `history.rs` log and machines; `ownership.rs` claim,
  takeover and release, the claim step `update` shares, and `command_signature`, the receipt).
- `tests/cli/main.rs`: the one test binary's root; `sandbox()` hands test support the binary Cargo
  built (`env!("CARGO_BIN_EXE_bead")`), the only place that default can come from.
- `tests/cli/commands/*.rs`: command behavior, and `commands/setup.rs`, the golden
  path of setting a tracker up (roll-dih8.13); `tests/cli/golden.rs` and
  `golden/normal.rs`: the golden scenarios and their normalisation, against the files in
  `tests/golden/`, fixed reference outputs whose provenance is in `docs/REVIEWING.md` §2.
- `tests/cli/parity.rs` and `parity/*.rs`: machines converge and export alike, the published tree
  follows format 1, sync settles pending attempts, a retry recognises its receipt
  (`parity/retries.rs`), and the answers
  pinned by reference fixtures: the receipts' bytes (`parity/receipts.rs`),
  the unpublished counts (`parity/pending.rs`), and what bead says to arguments, actors and paths
  that are not UTF-8 (`parity/inputs.rs`, `parity/actors.rs`).
- `tests/cli/protocol.rs`, `hardening.rs`, `storage.rs`, `compat.rs`, `replay.rs`: the black-box
  tests of their respective subjects (the real-export test is `#[ignore]`d, run by
  `cargo xtask check --real`); `hardening/gate.rs`: the `pause-until` handshake a test holds bead
  at. `tests/cli/sweep.rs`: the ownership protocol enumerated, its boundaries reached through
  failpoints (`fault_env`), each
  schedule showing that its fault fired (`fault_fired`); the thirteen-schedule slice runs in the
  fast suite, and `sweep/sweep_failure_reporting.rs` checks
  that a failed schedule fails it, and one whose fault never fired.
  `tests/cli/stress.rs`: two randomised models, ignored by default; they draw from
  `stress/seeded_random.rs`, whose MT19937 stream preserves recorded scenarios for each seed.
- `main.rs`'s `rust_only`: the machine of a test that uses build-specific failpoints, reads the
  store in the test process, or checks a build-specific contract (a structured error where Python
  died with a traceback). It runs this build's binary whatever `BEAD_UNDER_TEST` says.

## Edit here for X

- A test of a command: `tests/cli/<module>/<subject>.rs` beside related tests. Test names describe
  the contract they check; a regression test names its finding. Update active references in
  `docs/FINDINGS.md` when renaming a test.
- A command, its output or its exit status: `src/commands` and `src/cli`.
- An argument or a help text: `src/cli/spec.rs`. The help pages are goldens
  (`tests/golden/help.txt`): a deliberate change edits the golden by hand in the
  same change. An argument of a coordinated command is part of its receipt (`Namespace::asked`):
  `parity::receipts`, which pins the format bytes, must stay green.
- A golden scenario: `tests/cli/golden.rs`. For an intentional output change, edit the reference
  file in `tests/golden/` by hand in the same change and explain the new behavior.

## Invariants

- Only `cli` reads the process environment, the working directory and the clock; it passes values
  down.
- A command hands the one thing it is done with to its `Outcome` (`Outcome::held`): the state it
  answers from (`Outcome::answered_from`, its last state, never beside another), or the transaction
  a write was made in (`Outcome::wrote_in`, from `beadroll_sync::writing_spent`: `create`,
  `comments add`, `dep add`/`remove`, `close`/`reopen`/`delete` and `import`). `run` (the process)
  keeps it until exit rather than freeing it, a quarter of a reading command's time on a large
  tracker and 83 ms of a write's at 200,000 events, and frees it first when the run publishes what
  the command wrote; `run_with` (xtask, tests) frees it. A command that publishes or reads a state
  of its own before it answers lets go of its transaction before either (`transition`, when it ends
  a claim), so that none is held beside another. Library code never leaks (`docs/REVIEWING.md` §2,
  roll-lqjj.32, roll-lqjj.42).
- `src/cli/ending.rs` and `src/commands/held.rs` are unit tests that run `finish` and the commands
  in this process, on a machine of the sandbox opened by `src/testing.rs`.
- No `print!`, `println!` or `eprintln!`: every write goes through `cli/output.rs`, where a closed
  pipe or a hung-up terminal is a quiet exit 0 and a failed write to stderr keeps the status.
- Output preserves its established formats: `--json` results spaced with raw UTF-8, errors and
  `--version` ASCII-escaped, human lines with control characters escaped.
- Preserve accepted argument syntax, diagnostics and help wrapping at every width. Arguments
  stay `OsString` until a command reads them; invalid UTF-8 is rejected only where the command's
  contract requires valid text.
- Golden outputs and `parity` fixtures are fixed references; never regenerate them to hide a
  failure. Only an intentional, documented behavior change alters one, by hand.
- Test roots start with `#![cfg(test)]`; helpers shared between test modules are `pub(crate)`.
- Every test that does not need this build's own binary (`rust_only`) runs whatever bead
  `BEAD_UNDER_TEST` names, so the suite can check an installed bead or a supported older release.
  Fault-injection tests must reach the intended operation and assert that their failpoint fired.

## Common commands

- `cargo nextest run -p beadroll`: the black-box tests against the binary this package builds
- `BEAD_UNDER_TEST=/abs/path/bead cargo nextest run -p beadroll`: the same against another bead
- `cargo nextest run -p beadroll -E 'test(/^golden::/) | test(/^parity::/)'`: the bead against
  the fixed output and compatibility fixtures
- `BEAD_SWEEP=full cargo nextest run -p beadroll -E 'test(=sweep::sweep::sweep)'`: all 660
  schedules of the sweep, in parallel (about 11 minutes on 16 cores); `BEAD_SWEEP_GC=1` and
  `BEAD_SWEEP_CONCURRENT=1` select its variants
- `cargo nextest run -p beadroll --run-ignored only -E 'test(/^stress::/)'`: the stress models,
  8 to 20 minutes each
- `cargo install --locked --force --path crates/beadroll --root ~/.local`, from the checkout's
  root: install the binary as `~/.local/bin/bead` (`README.md`, *Install*)
