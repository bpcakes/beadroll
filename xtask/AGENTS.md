# xtask crate guide

## Purpose

`xtask` is the workspace's own tooling, run as `cargo xtask` (`docs/RUST-PORT.md` §4): `oracle`
runs the Python test suite against any `bead` binary, `inventory` maps every Python test to the
Rust tests that port it, and `timing` records how long a command took. Later phases add `diff`,
`golden` and `check`.

## Key entrypoints

- `src/main.rs`: the commands and their usage.
- `src/oracle.rs`, `python/oracle.py`, `oracle-skip.txt`: the oracle, its Python runner and the
  tests it leaves out.
- `src/inventory.rs`, `inventory.txt`: the inventory and its seed, one row per Python test.
- `src/timing.rs`: `timing`.
- `src/workspace.rs`: paths, failures and JSON strings for all of them.

## Edit here for X

- A Python test added, renamed or removed: `inventory.txt`, and `oracle-skip.txt` if it cannot run
  against another binary (the runner and the inventory both refuse a list that disagrees with the
  suite).
- A new Python test module: `FAST` or `SLOW` in `src/oracle.rs`.
- A ported test that does not follow the naming rule (a D-class replacement): `=>` in
  `inventory.txt`.

## Invariants

- The skip list holds only tests that cannot target another binary or run no bead; a test that
  bypasses `BEAD_UNDER_TEST` is fixed, not skipped.
- The oracle never touches a real store: each module gets a fresh `BEAD_HOME`.
- Unless the binary is this checkout's Python launcher, `BEADROLL_ORACLE_REFUSALS` is set and
  Python bead refuses to start, so a bypass fails instead of passing against Python.
- Timings are evidence and never gate anything (§4.7).
- No dependencies; it may print (its crate root allows it).

## Common commands

- `cargo xtask oracle --bead ./bead`: the suite against Python's launcher; must pass completely
- `cargo xtask oracle --report`: against the Rust binary, built first; reports only
- `cargo xtask inventory`
- `cargo xtask timing --label NAME -- COMMAND`
- `cargo test -p xtask` (the oracle's own tests need `python3` 3.11 or later)
