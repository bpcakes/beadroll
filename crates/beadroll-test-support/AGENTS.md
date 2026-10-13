# beadroll-test-support crate guide

## Purpose

`crates/beadroll-test-support` supplies isolated test environments: a `Sandbox` (a temporary
root with a bare `origin.git` whose `main` has one commit) and `Machine`s (each with its own home,
clone and identity) that run a `bead` binary for every crate's black-box tests. Fixtures include
hooks, fake `ssh`, `ps` and `git`, PTYs and failpoint handshakes. For beadroll-core's integration
tests it builds events.

## Key entrypoints

- `src/sandbox.rs`: `Sandbox`, `git`, `select_bead`, the choice between `BEAD_UNDER_TEST` and the
  caller's default (`Sandbox::selecting` takes the variable's value instead of reading it), and
  `built_bead`, the default of the tests of a crate other than `beadroll`: the `bead` Cargo built in
  the profile directory above the running test (`<target>/<profile>/bead` beside
  `<target>/<profile>/deps/`).
- `src/machine.rs`: `Machine` (`run`, `bead`, `show`, `ready`, `export`, `store`, `with_bead`),
  `Invocation` (a run's directory, thread, agent, environment and input), `Run`, `code`.
- `src/lib.rs`: `Json` (beadroll-core's `Value`) and `parse_json`, which read `bead --json`.
- `src/events.rs`: the `json!` macro, `Event` (an event built as `Tx.emit` writes it), `id`,
  `id_at`, `journal` and `state`, for beadroll-core's integration tests only: core's own unit tests
  would build a second `beadroll_core` (E0308).

## Edit here for X

- Shared fixtures for isolated test environments: here; subject-specific helpers stay beside
  their tests.
- How a machine's environment is built: `Machine::env`.

## Invariants

- The default binary is always the caller's: this crate never expands
  `env!("CARGO_BIN_EXE_bead")`, which Cargo defines only for the integration tests of the package
  that builds `bead`. A crate other than `beadroll` passes `built_bead()`. It builds on its own:
  `cargo build -p beadroll-test-support`.
- A machine's binary is its own override, else `BEAD_UNDER_TEST`, else the supplied default; each
  must be an absolute path. The sandbox's `bead` must exist: a sandbox fails its test at once when
  it does not, saying how to build it, so that a test never fails command by command on a missing
  binary.
- A sandbox's bare repositories never maintain themselves (`receive.autogc` false): after a push,
  git's auto maintenance would repack a remote in the background, whenever its sample of loose
  objects (`objects/17`, times 256) passes a threshold, and pack the objects a test is about to
  inspect or damage (roll-k2wy.20). A test that wants a repack runs it.
- A run's environment is the test's, less every `CLAUDE*`, `CODEX*`, `BEAD_*` and `GIT_*`
  variable, then the machine's home and identity, then its extras.
- Nothing outside the sandbox is written; dropping it removes it.
- A failed fixture is a failed test: the crate root allows `unwrap`, `expect`, `panic!` and
  `eprintln!` (`CONTRIBUTING.md`).

## Common commands

- `cargo test -p beadroll-test-support`
- `cargo build -p beadroll-test-support`
