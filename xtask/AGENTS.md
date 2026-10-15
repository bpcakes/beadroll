# xtask crate guide

## Purpose

`xtask` is the workspace's own tooling, run as `cargo xtask`: `check` runs the workspace's
baseline, its tests, its doctests and the `infra/` drivers' tests, with `--stress` the stress
models and with `--real` the real-export checks, and `./check.sh` runs it; `timing`
records how long a command (or a replay) took; `bead` runs bead in xtask's process, whose
allocator counts its heap. Its tests also keep the repository hygiene rules and CI matrix.

## Key entrypoints

- `src/main.rs`: the commands and their usage.
- `src/check.rs`: `check`, its steps and how each is reported, and how it builds `bead` for
  `--real`; `check/check_script.rs` runs it with fake tool executables; `check/tests.rs`: its
  options, summaries and build.
- `src/timing.rs`, `src/timing/replay.rs`: `timing`, and `timing --replay N`; `timing/sync.rs`:
  `timing --sync N`, a publishing `bead sync` on a scratch store of the same history, with the
  store's memo of its journal files and its projection, without its projection, and without
  either (roll-lqjj.1, roll-lqjj.2); `timing/read.rs`: `timing --read N`, the reading commands
  there with the projection and without it, and the bytes the projection and its text file hold; `timing/merge.rs`: `timing --merge N`, a sync that
  merges what another machine wrote after the store's frontier; `timing/contend.rs`: `timing
  --contend M`, machines writing one tracker at once (roll-lqjj.7); `timing/import.rs`: `timing
  --import N`, importing a generated Beads export (roll-lqjj.9); `timing/plan.rs`: `timing --plan
  N`, the core calls an import of N records makes, every record depending on a key at the end of
  the batch (roll-lqjj.15); `timing/extend.rs`: `timing --extend N`, events applied to a projection
  of N issues one event at a time and in one batch, including a batch that changes one issue's
  labels over and over and writes whose ids sort before every id held (roll-lqjj.17, V01 and V02);
  `timing/dangling.rs`: `timing --dangling N`, replay of N creates whose imports name keys no issue
  has yet, never, later, or one issue every other's, and the controls, and a projection taking one
  issue's keys one create a batch (roll-lqjj.18, W01); `timing/ready.rs`: `timing --ready N`,
  readiness judged on N issues in a chain held by its root's prerequisite, the same chain free, a
  lattice whose rows share their ancestors, and that lattice beneath gates of its own for its first
  rows, for every issue at once (the analysis alone, and `State::ready` with its sort), issue by
  issue, and by the complete blocker lists made issue by issue (roll-lqjj.19) and all at once
  (`State::blocker_lists`, roll-lqjj.24); `timing/details.rs`: `timing --details N`, every issue of
  N on a sparse graph without parents shown in full, the issues that depend on each found once for
  all of them (`State::details_of`) and for each alone, and the reverse lookup alone both ways, with
  one issue's detail and lookup both ways as a control (roll-lqjj.20); `timing/renames.rs`:
  `timing --renames N`, replay of two machines each creating the same N keys, so that N copies are
  renamed, against the same creates renamed by nobody, and each with the first machine's issues
  importing a dependency on a key that never comes (roll-lqjj.25, X03); `timing/growth.rs`: the
  growth guards only time can keep (roll-lqjj.34, X15), tests ignored by every other run that time
  planning an import, its dry run, a re-import, replay of imports that wait for keys and a
  projection taking ids before every id it holds, at a size and eight times it, on the probes'
  shapes, and fail above 24 times as long;
  `timing/world.rs`: the scratch
  store, checkout and remote they run `bead` on; `timing/history.rs`: the histories it replays
  (`--ownership`: heavy in claims, takeovers and releases in attempt files; `--churn`: a thousand
  issues changed over and over, a state that stays its size); `timing/memory.rs`: the journal written under `target/timing/` while it is measured,
  `timing --load DIR` (the process that loads and replays it), `bead` (bead's own code run in
  xtask, for its heap), and their peaks: the resident set as
  `/usr/bin/time` reports it, and the heap as `timing/heap.rs` counts it, xtask's global allocator
  (the system's, counted; its one unsafe spot, `CONTRIBUTING.md`).
- `src/hygiene.rs`: the repository lint (a test only).
- `src/ci.rs`: `v8_supported_environments`, the CI matrix the workflows keep, and
  `x15_growth_guards`, the nightly step that runs the growth guards (tests only).
- `src/workspace.rs`: paths, failures and JSON strings for all of them.
- `python/printable_table.py`: writes `crates/beadroll-core/src/text/printable.rs` with the Unicode
  non-printable table that `repr()` uses, generated with CPython 3.14.7's `str.isprintable()`
  under Unicode 16.0.0.

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
- Timings are evidence and never gate anything, with one exception, the growth guards
  (`timing/growth.rs`, X15): a ratio of two timings of one core call, the larger input eight times
  the smaller, may not exceed 24, where growing with the input gives about 8 and with its square
  64. They are `#[ignore]`d, so that no debug build, `./check.sh` or pull request is gated by a
  timing, and the nightly workflow runs them in release, one at a time:
  `cargo nextest run --release --locked --package xtask --run-ignored only --test-threads 1 -E
  'test(/^timing::growth::/)'`; `ci::x15_growth_guards` keeps that step. Anything a count can see
  is guarded by count instead, in beadroll-core (`work::tests`), on every push.
- `timing --sync` runs `bead` and git on a scratch store, checkout and remote under
  `target/timing/`, which it removes after, as workspace `timing`, never the real store or another
  repository: none of them inherits the shell's `BEAD_*` and `GIT_*` variables.
- Its dependencies are beadroll-core and the `beadroll` library (for `bead`, which runs bead's code
  in xtask's process), and for its tests regex-lite (the hygiene patterns),
  sha2 (the plain history's digest) and tempfile; it may print (its crate root allows it).

## Common commands

- `cargo xtask check [--stress] [--real]`, or `./check.sh` with the same options: the workspace's
  baseline, one line per step
- `cargo xtask timing --label NAME -- COMMAND`
- `cargo run --release --package xtask -- timing --replay 10000`, and with `--ownership`
- `cargo run --release --package xtask -- timing --sync 200000`, and with `--ownership` or
  `--rotate BYTES`
- `cargo run --release --package xtask -- timing --import 14000`: a generated 49 MB export,
  offline, its first sync, and published, and the projection and text file it made
- `cargo run --release --package xtask -- timing --plan 16000`: planning that import, validating
  it for a dry run, and re-importing a known issue whose dependencies are all unresolved
- `cargo run --release --package xtask -- timing --extend 8000`: closes, label additions and
  dependency additions applied to a projection of 8,000 issues, one event at a time and in one
  batch, with one issue's labels changed over and over and writes whose ids sort before every id
  held
- `cargo run --release --package xtask -- timing --ready 4000`: readiness on a held chain, a free
  chain, a lattice and a gated lattice of 4,000 issues, judged for every issue at once, issue by
  issue, and by the complete blocker lists made issue by issue and all at once
- `cargo run --release --package xtask -- timing --details 8000`: every issue of 8,000 shown in
  full, its dependents found once for all of them and for each alone, and one issue's as a control
- `cargo run --release --package xtask -- timing --renames 8000`: replay renaming 8,000 copies two
  machines minted alike, against 16,000 creates renamed by nobody
- `cargo test -p xtask`
- `cargo nextest run --release --locked --package xtask --run-ignored only --test-threads 1 -E
  'test(/^timing::growth::/)'`: the growth guards, as the nightly workflow runs them
- `python3 xtask/python/printable_table.py > crates/beadroll-core/src/text/printable.rs`, with
  CPython 3.14.7 (Unicode 16.0.0), the versions the table was made with; do not change the
  Unicode version implicitly
