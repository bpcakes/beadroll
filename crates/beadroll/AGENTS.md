# beadroll crate guide

## Purpose

`crates/beadroll` is the `bead` binary (`docs/RUST-PORT.md` §2.1): `commands` will map typed
requests to typed outcomes, `cli` will parse arguments, render results and own the output streams
and exit codes, and `sys` will hold the two audited unsafe spots. Phase 0 implements no command:
`run` refuses every invocation with exit status 1. Its integration tests are the black-box tests
ported from the Python suite.

## Key entrypoints

- `src/main.rs`: the executable; it calls `beadroll::run`.
- `src/lib.rs`: `run`.
- `tests/cli/main.rs`: the one test binary's root; `sandbox()` hands test support the binary Cargo
  built (`env!("CARGO_BIN_EXE_bead")`), the only place that default can come from.
- `tests/cli/commands.rs`, `tests/cli/commands/*.rs`: `tests/test_commands.py`, ported.
- `tests/cli/support.rs`: which binary the tests run.

## Edit here for X

- Port a black-box Python test: `tests/cli/<module>/<class>.rs`, named by the inventory's rule
  (`xtask/inventory.txt`): `test_commands.HelpAndPrime.test_x` becomes
  `commands::help_and_prime::x`. It counts once it passes against Python's `./bead`.
- A command, its output or its exit status: `src/commands` and `src/cli`, from Phase 2.

## Invariants

- Only `cli` reads the process environment, the working directory and the clock; it passes values
  down (§2.2).
- No `print!`, `println!` or `eprintln!`: every write goes through the output module, where a
  closed pipe is a quiet exit 0 (§2.3f).
- Test roots start with `#![cfg(test)]`; helpers shared between test modules are `pub(crate)`.
- A ported test passes against Python's launcher before it counts; Rust passes it by Phase 2d.

## Common commands

- `BEAD_UNDER_TEST=$PWD/bead cargo nextest run -p beadroll`: the black-box tests against Python
- `cargo nextest run -p beadroll`: the same against the Rust binary, which fails them until Phase 2
- `cargo xtask oracle --report`: the Python suite against the Rust binary
