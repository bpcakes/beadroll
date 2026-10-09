# beadroll crate guide

## Purpose

`crates/beadroll` is the `bead` binary (`docs/RUST-PORT.md` §2.1): `commands` maps typed requests to
typed outcomes, `cli` parses arguments, renders results and owns the output streams and exit codes.
Slice 2a (`roll-k2wy.3`) ports `init`, `create`, `show`, `list`, `ready` and `sync`; the parser does
not know the other commands until Slices 2b and 2c add them, and Slice 2d adds `sys`, the second
audited unsafe spot. Its integration tests are the black-box tests ported from the Python suite and
the mixed-fleet tests.

## Key entrypoints

- `src/main.rs`: the executable; it calls `beadroll::run`.
- `src/cli.rs`: the flow of one run (`--json`, `--actor` and `--` read first, settings, the
  workspace, the command, publishing what it wrote, the unpublished count, rendering, errors);
  `src/cli/args.rs`: the clap parser; `src/cli/output.rs`: the EPIPE-safe writes and `visible`;
  `src/cli/texts.rs`: the overview.
- `src/commands.rs`: `Failure`, `Outcome` and the helpers commands share (`field_args`, `select`);
  `src/commands/*.rs`: one command each (`import.rs` is `init --import`'s import).
- `tests/cli/main.rs`: the one test binary's root; `sandbox()` hands test support the binary Cargo
  built (`env!("CARGO_BIN_EXE_bead")`), the only place that default can come from.
- `tests/cli/commands/*.rs`: `tests/test_commands.py`, ported; `tests/cli/mixed.rs`: Python and Rust
  machines on one remote and on one store.

## Edit here for X

- Port a black-box Python test: `tests/cli/<module>/<class>.rs`, named by the inventory's rule
  (`xtask/inventory.txt`): `test_commands.HelpAndPrime.test_x` becomes
  `commands::help_and_prime::x`. It counts once it passes against Python's `./bead`.
- A command, its output or its exit status: `src/commands` and `src/cli`.

## Invariants

- Only `cli` reads the process environment, the working directory and the clock; it passes values
  down (§2.2).
- No `print!`, `println!` or `eprintln!`: every write goes through `cli/output.rs`, where a closed
  pipe or a hung-up terminal is a quiet exit 0 and a failed write to stderr keeps the status.
- Output keeps Python's dialects: `--json` results spaced with raw UTF-8, errors and `--version`
  ASCII-escaped, human lines with control characters escaped.
- Test roots start with `#![cfg(test)]`; helpers shared between test modules are `pub(crate)`.
- A ported test passes against Python's launcher before it counts; Rust passes it by Phase 2d.
  The mixed-fleet tests name both binaries themselves, whatever `BEAD_UNDER_TEST` says.

## Common commands

- `BEAD_UNDER_TEST=$PWD/bead cargo nextest run -p beadroll`: the black-box tests against Python
- `cargo nextest run -p beadroll`: the same against the Rust binary
- `cargo nextest run -p beadroll -E 'test(/^mixed::/)'`: the mixed-fleet tests
- `cargo xtask oracle --report`: the Python suite against the Rust binary
