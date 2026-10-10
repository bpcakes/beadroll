# beadroll-test-support crate guide

## Purpose

`crates/beadroll-test-support` is the Rust `tests/support.py` (`docs/RUST-PORT.md` §4.4): a
`Sandbox` (a temporary root with a bare `origin.git` whose `main` has one commit) and `Machine`s
(each with its own home, clone and identity) that run a `bead` binary, for the black-box tests of
every crate. Later phases add the fixtures the ported tests need: hooks, fake `ssh`, `ps` and
`git`, PTYs and failpoint handshakes. For beadroll-core's integration tests it builds events.

## Key entrypoints

- `src/sandbox.rs`: `Sandbox`, `git`, `select_bead`, the choice between `BEAD_UNDER_TEST` and the
  caller's default, and `select_mixed`, the second `bead` that `BEAD_MIXED_WITH` names for every
  second machine of a sandbox (`Sandbox::selecting` takes both values instead of reading them).
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
- `BEAD_MIXED_WITH`, set and not empty, names a second `bead` (an absolute path) for a mixed fleet
  (`docs/RUST-PORT.md` §4.2, item 4): every second machine a sandbox makes, the second, the fourth
  and so on in the order `Machine::new` makes them, runs it instead of the one above, unless it has
  its own override (`with_bead`, which `rust_only` uses). Unset or empty, it changes nothing. A
  sandbox counts its machines with an atomic, since tests make them from several threads.
- A run's environment is the test's, less every `CLAUDE*`, `CODEX*`, `BEAD_*` and `GIT_*`
  variable, then the machine's home and identity, then its extras.
- Nothing outside the sandbox is written; dropping it removes it.
- A failed fixture is a failed test: the crate root allows `unwrap`, `expect`, `panic!` and
  `eprintln!` (§2.2).

## Common commands

- `cargo test -p beadroll-test-support`
- `cargo build -p beadroll-test-support`
- `BEAD_UNDER_TEST=$PWD/bead BEAD_MIXED_WITH=$PWD/target/debug/bead cargo nextest run -p beadroll
  -E 'test(/^protocol::/)'`: the protocol scenarios on a mixed fleet, Python's machines first; the
  same with the two paths swapped puts Rust's first
