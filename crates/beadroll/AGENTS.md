# beadroll crate guide

## Purpose

`crates/beadroll` is the `bead` binary (`docs/RUST-PORT.md` §2.1): `commands` maps typed requests to
typed outcomes, `cli` parses arguments, renders results and owns the output streams and exit codes.
Slices 2a and 2b (`roll-k2wy.3`, `roll-k2wy.4`) port every command but the coordinated ones:
`claim`, `release`, `takeover` and `update --claim`/`--status in_progress` parse, and answer
`USAGE` ("not in this build of bead yet") until Slice 2c adds them; Slice 2d adds `sys`, the
second audited unsafe spot. Its integration tests are the black-box tests ported from the Python
suite, the golden outputs and the mixed-fleet tests.

## Key entrypoints

- `src/main.rs`: the executable; it calls `beadroll::run`.
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
  `queries.rs` blocked, search and lint; `history.rs` log and machines).
- `tests/cli/main.rs`: the one test binary's root; `sandbox()` hands test support the binary Cargo
  built (`env!("CARGO_BIN_EXE_bead")`), the only place that default can come from.
- `tests/cli/commands/*.rs`: `tests/test_commands.py`, ported; `tests/cli/mixed.rs`: Python and Rust
  machines on one remote and on one store; `tests/cli/golden.rs` and `golden/normal.rs`: the
  golden scenarios and their normalisation, against the files in `tests/golden/`.

## Edit here for X

- Port a black-box Python test: `tests/cli/<module>/<class>.rs`, named by the inventory's rule
  (`xtask/inventory.txt`): `test_commands.HelpAndPrime.test_x` becomes
  `commands::help_and_prime::x`. It counts once it passes against Python's `./bead`.
- A command, its output or its exit status: `src/commands` and `src/cli`.
- An argument or a help text: `src/cli/spec.rs`, as `beadroll/cli.py` has it; then
  `cargo xtask golden`, which captures the help pages from Python.
- A golden scenario: `tests/cli/golden.rs`, then `cargo xtask golden`; never edit a file in
  `tests/golden/` by hand.

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
- Goldens come from Python only (`cargo xtask golden` refuses any other binary); a golden is never
  regenerated to make Rust pass.
- Test roots start with `#![cfg(test)]`; helpers shared between test modules are `pub(crate)`.
- A ported test passes against Python's launcher before it counts; Rust passes it by Phase 2d.
  The mixed-fleet tests name both binaries themselves, whatever `BEAD_UNDER_TEST` says.

## Common commands

- `BEAD_UNDER_TEST=$PWD/bead cargo nextest run -p beadroll`: the black-box tests against Python
- `cargo nextest run -p beadroll`: the same against the Rust binary
- `cargo nextest run -p beadroll -E 'test(/^mixed::/)'`: the mixed-fleet tests
- `cargo nextest run -p beadroll -E 'test(/^golden::/)'`: the Rust binary against the goldens
- `cargo xtask golden`: capture the goldens from Python's `./bead`
- `cargo xtask oracle --report`: the Python suite against the Rust binary
