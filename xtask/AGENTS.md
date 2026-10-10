# xtask crate guide

## Purpose

`xtask` is the workspace's own tooling, run as `cargo xtask` (`docs/RUST-PORT.md` §4): `oracle`
runs the Python test suite against any `bead` binary, `inventory` maps every Python test to the
Rust tests that port it, `timing` records how long a command (or a replay) took, `diff` checks
beadroll-core against CPython case by case, and `golden` captures the golden outputs from Python's
launcher. `check` runs the workspace's baseline: its tests, its doctests and the `infra/` drivers'
tests, with `--stress` the stress models and with `--real` the real-export checks.

## Key entrypoints

- `src/main.rs`: the commands and their usage.
- `src/oracle.rs`, `python/oracle.py`, `oracle-skip.txt`: the oracle, its Python runner and the
  tests it leaves out.
- `src/inventory.rs`, `inventory.txt`: the inventory and its seed, one row per Python test.
- `src/timing.rs`, `src/timing/replay.rs`: `timing`, and `timing --replay N`; `timing/history.rs`:
  the histories it replays (`--ownership`: heavy in claims, takeovers and releases in attempt
  files); `timing/memory.rs`: the journal written under `target/timing/` while it is measured,
  `timing --load DIR` (the process that loads and replays it), and peak memory as
  `/usr/bin/time` reports it.
- `src/diff.rs`: `diff replay` and `diff json`. `diff/report.rs` builds the report
  `python/replay_dump.py` builds from Python; `diff/generate/` draws histories (`model.rs`, then
  `hostile.rs` and `imports.rs` damage and import them; `scenarios.rs` holds fixed ones);
  `diff/json.rs` and `json/corpus.rs` against `python/json_dump.py`; `diff/run.rs` runs the
  drivers. `python/capture_fixtures.py` records the journals the Python tests build;
  `fixtures/replay/tests.jsonl` is the sample of them CI replays.
- `src/golden.rs`: `golden`, which runs the golden tests (`crates/beadroll/tests/cli/golden.rs`) in
  capture mode against `./bead`; the scenarios and their normalisation live in those tests.
- `src/check.rs`: `check`, its steps and how each is reported; `check_script` runs it with a fake
  `cargo` and `python3`, as `test_check.CheckScript` ran `check.sh`.
- `src/hygiene.rs`: `test_hygiene` ported, the repository lint (a test only).
- `src/workspace.rs`: paths, failures and JSON strings for all of them.

## Edit here for X

- A Python test added, renamed or removed: `inventory.txt`, and `oracle-skip.txt` if it cannot run
  against another binary (the runner and the inventory both refuse a list that disagrees with the
  suite). Since Phase 3 every Python test has its Rust port, and CI runs `inventory
  --require-complete`: a new Python test lands with its port, or a `=>` replacement.
- A new Python test module: `FAST` or `SLOW` in `src/oracle.rs`.
- A ported test that does not follow the naming rule (a D-class replacement): `=>` in
  `inventory.txt`.
- A golden output: the scenario in `crates/beadroll/tests/cli/golden.rs`, then `cargo xtask golden`.
- A step of the baseline: `steps` in `src/check.rs`; a test of an `infra/` driver: `infra/test_*.py`
  beside it, which `check.sh` and `check` both run.
- A Python behaviour the diff must cover: the generators in `src/diff/generate/` or the JSON
  corpus; something both reports must show: `report.rs` and `replay_dump.py` together.

## Invariants

- The skip list holds only tests that cannot target another binary or run no bead; a test that
  bypasses `BEAD_UNDER_TEST` is fixed, not skipped.
- The oracle never touches a real store: each module gets a fresh `BEAD_HOME`.
- Without `--bead`, the oracle tests the executable Cargo reports building, never a path it
  works out: the target directory, a build target and the profile can each move it.
- Unless the binary is this checkout's Python launcher, `BEADROLL_ORACLE_REFUSALS` is set and
  Python bead refuses to start, so a bypass fails instead of passing against Python.
- Timings are evidence and never gate anything (§4.7).
- Goldens are captured from Python's launcher only: the golden tests refuse to capture from any
  other binary, and a golden is never regenerated to make Rust pass.
- `diff` keeps no list of expected differences: a difference is fixed in Rust or decided as a row
  of `docs/RUST-PORT.md` §5 and applied to Python. Its drivers run under PYTHONINTMAXSTRDIGITS=4300
  and refuse any other limit.
- `check` gives every step a fresh scratch `BEAD_HOME` under `target/check/`, never the real
  store, and fails when any step fails, the later steps still running; a round trip that passes
  shows only its summary, one that fails everything it printed.
- Its dependencies are beadroll-core, proptest (which draws the generated cases) and sha2, and for
  its tests regex-lite (the hygiene patterns, as Python writes them) and tempfile; it may print
  (its crate root allows it).

## Common commands

- `cargo xtask check [--stress] [--real]`: the workspace's baseline, one line per step
- `cargo xtask oracle --bead ./bead`: the suite against Python's launcher; must pass completely
- `cargo xtask oracle --report`: against the Rust binary, built first; reports only
- `cargo xtask inventory [--require-complete]`: CI requires every Python test ported
- `cargo xtask golden`: the goldens, from Python's `./bead`
- `cargo xtask timing --label NAME -- COMMAND`
- `cargo run --release --package xtask -- timing --replay 10000`, and with `--ownership`
- `cargo xtask diff replay [--cases N] [--seed S] [--fixtures FILE]`, and `diff json` likewise
- `python3 xtask/python/capture_fixtures.py OUT.jsonl` (the whole suite, about 20 minutes), then
  `cargo xtask diff replay --cases 0 --fixtures OUT.jsonl`
- `cargo test -p xtask` (the tests of the oracle and of `diff json` need `python3` 3.11 or later)
