# xtask crate guide

## Purpose

`xtask` is the workspace's own tooling, run as `cargo xtask` (`docs/RUST-PORT.md` §4): `check` runs
the workspace's baseline, its tests, its doctests and the `infra/` drivers' tests, with `--stress`
the stress models and with `--real` the real-export checks, and `./check.sh` runs it; `timing`
records how long a command (or a replay) took; `bead` runs bead in xtask's process, whose
allocator counts its heap. Its tests also keep two of the Python suite's
properties that are about the repository rather than bead: the hygiene scan and the CI matrix.

Until the cutover it also ran the Python test suite against any `bead` (`oracle`), mapped every
Python test to its Rust port (`inventory`), checked beadroll-core against CPython case by case
(`diff`) and captured the goldens from Python's launcher (`golden`). They compared with or ran the
Python implementation, so they went with it; `python-0.1.0` has them.

## Key entrypoints

- `src/main.rs`: the commands and their usage.
- `src/check.rs`: `check`, its steps and how each is reported, and how it builds `bead` for
  `--real`; `check/check_script.rs` runs it with a fake `cargo` and `python3`, as
  `test_check.CheckScript` ran `check.sh`; `check/tests.rs`: its options, summaries and build.
- `src/timing.rs`, `src/timing/replay.rs`: `timing`, and `timing --replay N`; `timing/sync.rs`:
  `timing --sync N`, a publishing `bead sync` on a scratch store of the same history, with the
  store's memo of its journal files and its projection, without its projection, and without
  either (roll-lqjj.1, roll-lqjj.2); `timing/read.rs`: `timing --read N`, the reading commands
  there with the projection and without it; `timing/merge.rs`: `timing --merge N`, a sync that
  merges what another machine wrote after the store's frontier; `timing/contend.rs`: `timing
  --contend M`, machines writing one tracker at once (roll-lqjj.7); `timing/import.rs`: `timing
  --import N`, importing a generated Beads export (roll-lqjj.9); `timing/world.rs`: the scratch
  store, checkout and remote they run `bead` on; `timing/history.rs`: the histories it replays
  (`--ownership`: heavy in claims, takeovers and releases in attempt files; `--churn`: a thousand
  issues changed over and over, a state that stays its size); `timing/memory.rs`: the journal written under `target/timing/` while it is measured,
  `timing --load DIR` (the process that loads and replays it), `bead` (bead's own code run in
  xtask, for its heap), and their peaks: the resident set as
  `/usr/bin/time` reports it, and the heap as `timing/heap.rs` counts it, xtask's global allocator
  (the system's, counted; its one unsafe spot, `docs/RUST-PORT.md` §2.2).
- `src/hygiene.rs`: `test_hygiene` ported, the repository lint (a test only).
- `src/ci.rs`: `V8_Python312` rewritten, the CI matrix the workflows keep (a test only).
- `src/workspace.rs`: paths, failures and JSON strings for all of them.
- `python/printable_table.py`: writes `crates/beadroll-core/src/text/printable.rs` from CPython's
  own `str.isprintable()`, the table `repr()` escapes by.

## Edit here for X

- A step of the baseline: `steps` in `src/check.rs`; a test of an `infra/` driver: `infra/test_*.py`
  beside it, which `check` runs.
- A timing: `src/timing.rs`, or the histories in `timing/history.rs` (the plain one's digest is
  pinned, so that earlier records stay comparable).

## Invariants

- `check` gives every step a fresh scratch `BEAD_HOME` under `target/check/`, never the real
  store, and fails when any step fails, the later steps still running; a round trip that passes
  shows only its summary, one that fails everything it printed.
- Without `BEAD_UNDER_TEST`, `check --real` tests the executable Cargo reports building, never a
  path it works out: the target directory, a build target and the profile can each move it.
- Timings are evidence and never gate anything (§4.7).
- `timing --sync` runs `bead` and git on a scratch store, checkout and remote under
  `target/timing/`, which it removes after, as workspace `timing`, never the real store or another
  repository: none of them inherits the shell's `BEAD_*` and `GIT_*` variables.
- Its dependencies are beadroll-core and the `beadroll` library (for `bead`, which runs bead's code
  in xtask's process), and for its tests regex-lite (the hygiene patterns, as Python wrote them),
  sha2 (the plain history's digest) and tempfile; it may print (its crate root allows it).

## Common commands

- `cargo xtask check [--stress] [--real]`, or `./check.sh` with the same options: the workspace's
  baseline, one line per step
- `cargo xtask timing --label NAME -- COMMAND`
- `cargo run --release --package xtask -- timing --replay 10000`, and with `--ownership`
- `cargo run --release --package xtask -- timing --sync 200000`, and with `--ownership` or
  `--rotate BYTES`
- `cargo run --release --package xtask -- timing --import 14000`: a generated 49 MB export,
  offline, its first sync, and published
- `cargo test -p xtask`
- `python3 xtask/python/printable_table.py > crates/beadroll-core/src/text/printable.rs`, with
  the Python whose Unicode tables bead is to follow
