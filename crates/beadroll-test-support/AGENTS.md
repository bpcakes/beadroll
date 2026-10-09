# beadroll-test-support crate guide

## Purpose

`crates/beadroll-test-support` is the Rust `tests/support.py` (`docs/RUST-PORT.md` §4.4): a
`Sandbox` (a temporary root with a bare `origin.git` whose `main` has one commit) and `Machine`s
(each with its own home, clone and identity) that run a `bead` binary, for the black-box tests of
every crate. Later phases add the fixtures the ported tests need: hooks, fake `ssh`, `ps` and
`git`, PTYs and failpoint handshakes. For beadroll-core's integration tests it builds events.

## Key entrypoints

- `src/sandbox.rs`: `Sandbox`, `git`, and `select_bead`, the choice between `BEAD_UNDER_TEST` and
  the caller's default.
- `src/machine.rs`: `Machine` (`run`, `bead`, `show`, `ready`, `export`, `store`, `with_bead`),
  `Invocation` (a run's directory, thread, agent, environment and input), `Run`, `code`.
- `src/lib.rs`: `Json` (beadroll-core's `Value`) and `parse_json`, which read `bead --json`.
- `src/events.rs`: the `json!` macro, `Event` (an event built as `Tx.emit` writes it), `id`,
  `id_at`, `journal` and `state`, for beadroll-core's integration tests only: core's own unit tests
  would build a second `beadroll_core` (E0308).

## Edit here for X

- Something every ported test needs from its world: here, ported from `tests/support.py` or the
  test module's own helpers.
- How a machine's environment is built: `Machine::env`, which mirrors `Machine.env`.

## Invariants

- The default binary is always the caller's: this crate never expands
  `env!("CARGO_BIN_EXE_bead")`, which Cargo defines only for the integration tests of the package
  that builds `bead`. It builds on its own: `cargo build -p beadroll-test-support`.
- A machine's binary is its own override, else `BEAD_UNDER_TEST`, else the supplied default; each
  must be an absolute path.
- A run's environment is the test's, less every `CLAUDE*`, `CODEX*`, `BEAD_*` and `GIT_*`
  variable, then the machine's home and identity, then its extras.
- Nothing outside the sandbox is written; dropping it removes it.
- A failed fixture is a failed test: the crate root allows `unwrap`, `expect`, `panic!` and
  `eprintln!` (§2.2).

## Common commands

- `cargo test -p beadroll-test-support`
- `cargo build -p beadroll-test-support`
