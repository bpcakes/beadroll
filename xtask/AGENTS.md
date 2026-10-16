# xtask crate guide

## Purpose

`xtask` is the workspace's own tooling, run as `cargo xtask`: `check` runs the workspace's
baseline, its tests, its doctests and the `infra/` drivers' tests, with `--stress` the stress
models and with `--real` the real-export checks, and `./check.sh` runs it; `coverage` reports which
lines of the product's code the suites run, with cargo-llvm-cov; `timing`
records how long a command (or a replay) took; `bead` runs bead in xtask's process, whose
allocator counts its heap, the most it held and what it still holds when bead returns. Its tests
also keep the repository hygiene rules and CI matrix, and what bead holds on its heap.

## Key entrypoints

- `src/main.rs`: the commands and their usage.
- `src/check.rs`: `check`, its steps and how each is reported, and how it builds `bead` for
  `--real`; `check/check_script.rs` runs it with fake tool executables; `check/tests.rs`: its
  options, summaries and build.
- `src/coverage.rs`: `coverage [--stress] [--sweep]`: `Tools::require` (cargo-llvm-cov and the
  toolchain's llvm-tools installed, or the command that installs each), its `steps` (run by
  `check::run`: the old profiles cleaned, the suites through `cargo llvm-cov nextest --no-report`,
  then the HTML, lcov and JSON reports under `target/coverage/`) and `summarize` (lines by crate,
  the `PROTOCOL` files with the runs of lines no test reaches, and whether the black-box tests'
  `bead` processes are in the count: `CHILD_ONLY`), `unmerged_profiles` (the profiles the report
  steps' logs say were left out: the run fails on any); `coverage/lcov.rs` reads an `lcov.info`;
  `coverage/tests.rs` runs the command on fake tools.
- `src/timing.rs`, `src/timing/replay.rs`: `timing`, and `timing --replay N`; `timing/sync.rs`:
  `timing --sync N`, a publishing `bead sync` on a scratch store of the same history, with the
  store's memo of its journal files and its projection, without its projection, and without
  either (roll-lqjj.1, roll-lqjj.2), and a write published at once (roll-lqjj.29); `timing/read.rs`: `timing --read N`, the reading commands
  there (`log`, `info` and `machines` among them, roll-lqjj.31) with the projection and without it, a local write, and a write cycle (a write after a read, the read after it, which writes the projection back, and a write after a write: roll-lqjj.36), and the bytes the projection and its text file hold; `timing/merge.rs`: `timing --merge N`, a sync that
  merges what another machine wrote after the store's frontier; `timing/contend.rs`: `timing
  --contend M`, machines writing one tracker at once (roll-lqjj.7); `timing/import.rs`: `timing
  --import N`, importing a generated Beads export (roll-lqjj.9); `timing/plan.rs`: `timing --plan
  N`, the core calls an import of N records makes, every record depending on a key at the end of
  the batch (roll-lqjj.15); `timing/extend.rs`: `timing --extend N`, events applied to a projection
  of N issues one event at a time and in one batch, including a batch that changes one issue's
  labels over and over, writes whose ids sort before every id held (roll-lqjj.17, V01 and V02),
  and closes after an event of an operation a newer bead writes (roll-lqjj.28);
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
  importing a dependency on a key that never comes (roll-lqjj.25, X03); `timing/decode.rs`:
  `timing --decode N`, a projection of N events decoded in this process, its state part alone (what
  a command that only reads decodes) and whole with its index (what a write decodes), with the heap
  a decoded state holds and the bytes of the state part, the index and the texts (roll-lqjj.35,
  X16); `timing/growth.rs`: the
  growth guards only time can keep (roll-lqjj.34, X15), tests ignored by every other run that time
  planning an import, its dry run, a re-import, replay of imports that wait for keys, replay of
  copies two machines minted alike (`timing/renames.rs`'s journals; roll-lqjj.38, X03) and a
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
- `tests/heap/main.rs`: the one integration-test binary, bead's heap as `cargo xtask bead` counts
  it (roll-lqjj.42, X17): a sandbox machine whose `bead` is this build's xtask runs commands on a
  tracker of 1,500 imported issues, and the test reads `heap peak:` and `heap held:` from standard
  error. It keeps that a write published at once peaks where one not published does, that a close
  or delete that ends a claim peaks where the claim did (neither holds a transaction beside a
  state), and that every command, answering or failing, returns holding none of the tracker
  (`run_with` and the libraries free).
- `src/hygiene.rs`: the repository lint (a test only).
- `src/ci.rs`: `v8_supported_environments`, the CI matrix the workflows keep,
  `x15_growth_guards`, the nightly step that runs the growth guards, and `coverage_report`, the
  nightly job that makes the coverage report with the cargo-llvm-cov `CARGO_LLVM_COV` names and
  keeps it (tests only).
- `src/workspace.rs`: paths, failures and JSON strings for all of them, and `fake_tool`, the
  executable script the tests of `check` and `coverage` run in place of `cargo`.
- `python/printable_table.py`: writes `crates/beadroll-core/src/text/printable.rs` with the Unicode
  non-printable table that `repr()` uses, generated with CPython 3.14.7's `str.isprintable()`
  under Unicode 16.0.0.

## Edit here for X

- What the coverage report counts: `IGNORED` (files left out: the tooling and test scaffolding),
  `PROTOCOL` (the files whose unreached lines the summary lists), `CHILD_ONLY` (the file only a
  `bead` process runs) and `PROFILE` (the profile files' name) in `src/coverage.rs`; the
  cargo-llvm-cov version, `CARGO_LLVM_COV` there and in `rust-nightly.yml`, which
  `ci::coverage_report` keeps alike.
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
- `coverage` is evidence too: no minimum, no `--fail-under`, no comparison with an earlier report.
  It fails when a suite fails (the report is still made), when the report does not count the
  `bead` processes the black-box tests start, and when a profile was left out of it.
  `crates/beadroll/src/main.rs` can only be run by a `bead`, so a report where none of its lines
  ran lost the children (`beadroll-test-support` scrubs the environment `bead` runs in, `CLAUDE*`,
  `CODEX*`, `BEAD_*` and `GIT_*`, and must keep `LLVM_PROFILE_FILE`, which the instrumented `bead`
  writes its counters by). The reports are made with `--failure-mode all`, so that one profile that
  cannot be merged (a `bead` cut short while it wrote, or a pool file a capped run left truncated:
  every later process that picks that file is lost with it) does not cost the whole report, and
  llvm-profdata only warns of it in the report steps' logs; the command reads those warnings, says
  how many profiles were left out and which is the first, and fails, since counts lost read as
  code no test reaches. The report is of the command's own run: `llvm-cov clean --workspace` and
  the removal of the last outputs come first. The profiles are written to `PROFILE`, `%16m`, so
  that the processes of one binary merge into 16 files: the default, one file of half a megabyte
  for each process, is 3.5 GB for the fast suite alone.
- `coverage` has one count of lines, the one it prints and the docs quote: those of `lcov.info`,
  each source line with code once, run or not. The HTML index and `summary.json` are llvm-cov's own,
  which adds up the lines of each function, so that a line two functions share (a closure on the
  line of the call that takes it) counts twice: at 72b13f9, 96.12% of 21,018 lines there against
  96.4% of 19,705 here, with 1,313 lines more (`replay/apply/claims.rs` has 140 lines of code and
  164 there) and the closures that never ran among the uncovered. That is also what the line view
  cannot see: a closure that never ran on a line that did (`transition.rs:190`, roll-k2wy.14.15) is
  red in the HTML's regions and absent from the unreached lines printed. Both counts include the
  unit tests written inline in the product's files (`#[cfg(test)] mod tests { ... }`, 1,789 lines at
  99.6%, 11 of them in the protocol's files), which cargo-llvm-cov cannot leave out on a stable
  toolchain: the product's code alone is 96.1% of 17,916 lines. The tests in files of their own
  (`tests.rs`, `tests/`) are left out, as are the tooling and the test support (`IGNORED`).
- A `bead` that writes no profile leaves what only it runs reading as unreached: one killed by
  SIGKILL (the sweep's `sigkill` schedules, whose failpoint `kill_self` reads as unreached for it)
  and one run under a resource limit. Under `ulimit -f` the profile written at exit fails on the
  limit and says so on the standard error the test reads, and leaves a truncated file; under `ulimit
  -n` it may not open its file. So a test that runs `bead` under `ulimit` takes
  `beadroll_test_support::UNPROFILED` (`LLVM_PROFILE_FILE=/dev/null`) for that run, and
  `coverage::tests::limits` finds the runs under `ulimit` in the tracked test files (a string
  literal in code with `ulimit ` and `exec `, however it is written) and wants a use of it in code
  for each. The steps set `BEAD_UNDER_TEST` empty: a `bead` the build did not make has no counters.
  The doctests are left out: the workspace has none, and cargo-llvm-cov covers them only on a
  nightly toolchain.
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
  in xtask's process), and for its tests beadroll-test-support (the sandbox `tests/heap` runs bead
  in), regex-lite (the hygiene patterns), sha2 (the plain history's digest) and tempfile; it may
  print (its crate root allows it).
- `tests/heap` counts bytes, not time: the same command peaks at the same heap to within a few
  hundred bytes, so a margin of a twentieth is a whole projection's difference at the size of its
  tracker, and no gate on a clock. It runs `bead` through `Machine::with_bead`, whatever
  `BEAD_UNDER_TEST` names, since only xtask's own process counts.

## Common commands

- `cargo xtask check [--stress] [--real]`, or `./check.sh` with the same options: the workspace's
  baseline, one line per step
- `cargo xtask coverage [--stress] [--sweep]`: the fast suite under cargo-llvm-cov (1,026 tests in
  164 s here, 16 threads on a shared 64-core machine, 2 min 47 s for the whole command; 1,016 took
  172 s against 130 s plain); `--stress` adds the stress models (252 s) and `--sweep` all 660
  schedules (84 s), as the nightly workflow does (8 min 46 s together). On the nightly's 4-vCPU
  runner the same suites took about 3 minutes (the fast suite), 2 (stress) and 7 (sweep) without
  counters. Needs `cargo install --locked cargo-llvm-cov --version 0.8.7` and `rustup component
  add llvm-tools`. The report is `target/coverage/html/index.html`, with `lcov.info` and
  `summary.json` beside it. After a run, `cargo llvm-cov report --failure-mode all --text
  --output-path FILE` and `--show-missing-lines` read the same profiles further.
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
  batch, with one issue's labels changed over and over, writes whose ids sort before every id
  held, and closes after an event of an operation a newer bead writes
- `cargo run --release --package xtask -- timing --ready 4000`: readiness on a held chain, a free
  chain, a lattice and a gated lattice of 4,000 issues, judged for every issue at once, issue by
  issue, and by the complete blocker lists made issue by issue and all at once
- `cargo run --release --package xtask -- timing --details 8000`: every issue of 8,000 shown in
  full, its dependents found once for all of them and for each alone, and one issue's as a control
- `cargo run --release --package xtask -- timing --renames 8000`: replay renaming 8,000 copies two
  machines minted alike, against 16,000 creates renamed by nobody
- `cargo run --release --package xtask -- timing --decode 200000`, and with `--ownership`,
  `--churn` or `--text`: the projection of 200,000 events decoded, its state part alone and whole
- `cargo test -p xtask`, and `cargo nextest run -p xtask -E 'binary(heap)'` for bead's heap alone
- `cargo nextest run --release --locked --package xtask --run-ignored only --test-threads 1 -E
  'test(/^timing::growth::/)'`: the growth guards, as the nightly workflow runs them
- `python3 xtask/python/printable_table.py > crates/beadroll-core/src/text/printable.rs`, with
  CPython 3.14.7 (Unicode 16.0.0), the versions the table was made with; do not change the
  Unicode version implicitly
