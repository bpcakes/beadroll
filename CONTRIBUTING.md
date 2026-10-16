# Contributing

Thanks for looking. beadroll is small and deliberately boring: one binary, a
few dependencies, git. The rules below are the ones that keep it that way. It
was a Python package until 0.2.0 (the tag `python-0.1.0`).

## Setup

```sh
git clone https://github.com/bpcakes/beadroll && cd beadroll
cargo build --package beadroll   # the working copy, as target/debug/bead
./check.sh                       # the baseline: cargo xtask check, about 4 minutes
```

rustup (which installs the pinned toolchain, `rust-toolchain.toml`, 1.99.0, on
first use), cargo-nextest, git 2.40+, and Python 3 for the tests of the
`infra/` drivers. `cargo install --locked --force --path crates/beadroll --root
~/.local`, run from the checkout's root, installs the working copy as the
`bead` command if you want it on your PATH.

## The one rule about stores

The tests build their own repositories, bare remotes and machine homes in a
temporary directory and give every machine a scratch `BEAD_HOME`; they never
touch the default store under `~/.local/state/beadroll`. **Do the same by
hand**: before running `bead` to try something, `export BEAD_HOME=$(mktemp -d)`,
and run it only inside a repository you made for the experiment. In any other
checkout the origin URL routes you to that repository's real tracker. Never
push test data anywhere but a throwaway remote of your own.

## Layout

- `crates/` — six crates that depend only downwards: `beadroll-core` (pure:
  JSON, events, replay, queries, import and export) and `beadroll-git` (the
  only code that starts git) → `beadroll-store` → `beadroll-sync` → `beadroll`
  (the `bead` binary), and `beadroll-test-support`, the sandbox the tests run
  `bead` in. A helper two crates need moves down, never up. Each crate's
  `AGENTS.md` says what it owns.
- Tests: unit tests beside the code, and one integration-test binary per crate
  (`crates/<crate>/tests/<name>/`), by subject (`replay`, `storage`, `compat`,
  `commands`, `protocol`, `hardening`; the slow `stress` and `sweep`). The
  goldens in `crates/beadroll/tests/golden/` are what Python's bead printed.
- `xtask/` — `cargo xtask check` (the baseline), `cargo xtask coverage` (which lines the suites
  run) and `cargo xtask timing`.
- `infra/` — stdlib-Python drivers against real machines, with their tests.
- `README.md` is what the tool does, and `docs/GUARANTEES.md` every guarantee it
  keeps; `docs/REVIEWING.md` is the map of the design and the list of deliberate
  decisions; `docs/FINDINGS.md` is the ledger of every review finding and how it
  was closed.

## The tests

The tests are the Cargo workspace's, one integration-test binary per crate, organised by subject
as the Python suite they were ported from was, test by test: `replay` (causality, registers,
quarantine, validation, cycles; in beadroll-core, in-process and fast), `protocol` (claims,
attempts, fences, lost acknowledgements, sync races), `storage` (segments, torn tails, identity,
readers against writers), `commands` (every verb end to end, identity detection, branch
tracking), `compat` (Beads import/export, real exports, `br`/`bv`), `hardening` (the base under
the protocol: git environment and remotes, URLs and credentials, store files and modes, output
streams, imports), `stress` (randomised models, run by `--stress`), and `sweep` (the ownership
protocol enumerated: three machines, nine operations at seven fault boundaries, every ordered
pair of follow-ups, each schedule showing that its fault fired; a thirteen-schedule slice by
default, including retries of interrupted owner closes and deletes and a sync stopped at its
push; `BEAD_SWEEP=full` for all 660, with no failure exemptions). `golden` compares
every help page and the reading commands' output with Python's, captured before the cutover, and
`parity` holds what the mixed Python and Rust fleets checked, among it receipts byte for byte as
Python wrote them. `crates/beadroll-test-support` holds the sandbox: every test builds its own
repository, bare remotes and machine homes in a temporary directory, each machine with a scratch
`BEAD_HOME`, so the default store is never touched.

`cargo xtask coverage` says which lines of the product's code the suites run, the `bead` processes
the black-box tests start included, and which lines of the ownership protocol none does: an HTML
report, an `lcov.info` and a `summary.json` under `target/coverage/`. It needs cargo-llvm-cov
and the toolchain's `llvm-tools` (`rust-toolchain.toml` has neither), which it asks you to install
when they are missing; `--stress` and `--sweep` add the slow suites, as the nightly workflow does,
whose report is kept as an artifact. A test that runs `bead` under `ulimit` takes
`beadroll_test_support::UNPROFILED` (`LLVM_PROFILE_FILE=/dev/null`) for that run, since the profile
`bead` writes at exit would fail on the limit. The report is evidence, never a gate.

Every test that runs bead runs the one Cargo builds, unless `BEAD_UNDER_TEST` names another by its
absolute path. Until 0.2.0 bead was a Python package, whose suite the Rust tests port; the
port's tests passed against both, and the last Python is tagged `python-0.1.0`.
The plan the port followed, `docs/RUST-PORT.md`, is in git history (`git show
5a6ea8d:docs/RUST-PORT.md`), its measurements among it.

`infra/` holds the maintainer's harness against real machines and a throwaway GitHub
repository, stdlib Python that drives any `bead`:
`drive.py` (cross-machine scenarios over ssh), `soak.py` (random agent-like traffic),
`roundtrip.py` (import/export over real Beads exports), `selected.py` (which bead each runs, and
the record of it before any test data is written), their tests beside them, and the synthetic
`seed.jsonl`.

## Making a change

1. Every behaviour change comes with a test in the module of its subject. A
   fault is injected through a failpoint (`BEAD_FAILPOINTS`; the points and
   their actions are listed in `crates/beadroll-store/src/failpoint.rs`), never
   by patching: to skip the wait between sync attempts, `backoff@always=skip`,
   never a shortened sleep. Black-box tests run `bead` through
   `beadroll-test-support`'s `Sandbox` and `Machine`.
2. Match the surrounding style: comments in prose that say why; names that
   say what a function returns; errors carry a code a user or an agent can
   act on (`bead prime` lists them). Unknown input is reported, never guessed
   at; nothing is ever half-written. Keep the rules below.
3. Format 1 is frozen: a change to what events or the published manifest
   contain must be additive, so that an older bead sets the new thing aside
   rather than misreads it (*Format and upgrades* in `docs/GUARANTEES.md`).
4. A deliberate decision goes in `docs/REVIEWING.md` §2; a guarantee in
   `docs/GUARANTEES.md`; what a new user reads first in `README.md`. Keep the
   two agent texts (`bead prime`, `bead onboard`) true:
   `AGENTS.md` carries the onboard block verbatim and a test checks that.
5. Run `./check.sh` before opening a pull request; CI runs the tests on Linux
   and macOS, with the lints of `.github/workflows/rust.yml` (`AGENTS.md` lists
   them). Run `./check.sh --stress` when you touched the sync protocol.
6. Commit messages say what changed in behaviour, in prose; the subject line
   is the change, not the file.

## Rules every crate keeps

- **No ambient state in libraries.** Only the `beadroll` crate's `cli` reads
  the process environment, the working directory and the clock. It builds
  explicit values (`Settings`, the actor, the checkout path, the environment
  git children inherit) and passes them down, which is what lets studio and
  in-process tests use the crates; `std::env::set_var` is `unsafe` in edition
  2024 besides, so tests set environments on child processes only. The actor's
  one impure step, the walk up the process tree with `ps` when both a Claude
  and a Codex session variable are set, goes through a function the CLI passes
  in, so a fake `ps` on `PATH` drives its test.
- **Errors.** Each crate has precise `thiserror` enums whose variants are only
  those its operations can produce, and each error has a `code()`, one of the
  closed set `bead prime` lists. An OS error is wrapped with its path where it
  happens and reads `"{strerror}: {path}"`, without Rust's `(os error N)`; a
  failed rename names its target.
- **No panics outside tests.** Libraries deny `unwrap_used`, `expect_used`,
  `panic`, `todo` and `unimplemented`; indexing that cannot be shown safe uses
  `get`. Replay is total: every input is applied or reported, never an abort.
- **Determinism.** No `HashMap` or `HashSet` iteration order reaches output, a
  file or an event. Issues keep replay order, which `detail.dependents` and
  other outputs depend on; objects keep the order they were written in, and
  anything else is sorted explicitly.
- **No `print!`, `println!` or `eprintln!` in the product crates**: they panic
  on `EPIPE` and `EIO`. The CLI writes through one output module, which turns
  a broken pipe or a hung-up terminal into a quiet exit 0. Tests may print;
  xtask, a developer tool, allows it at its crate root.
- **Exemptions are crate-level attributes with a reason.** Cargo refuses
  `[lints]` entries beside `workspace = true` in a member. Crate-wide policy
  uses `allow`, not `expect`, which fails once the lint stops firing.
  `beadroll-test-support` is a library, so clippy's `allow-*-in-tests`
  settings do not reach it: its root allows `unwrap_used`, `expect_used`,
  `panic` and `print_stderr` ("a failed fixture is a failed test").
- **Every crate root has `//!` docs**, binaries and test roots included
  (`missing_docs` fires on them). Each `tests/<name>/main.rs` starts with
  `#![cfg(test)]`, so that clippy treats its helpers as test code, and helpers
  shared between test modules are `pub(crate)` (`unreachable_pub`). A root at
  `tests/<name>.rs` would look for its modules beside it (E0583), hence
  `main.rs`; one test binary per crate keeps linking short. Slow tests are
  `#[ignore = "reason"]`.
- **Unsafe code** is denied workspace-wide, with three exceptions, each block
  with a `// SAFETY:` comment: `beadroll::sys` (a probe of standard input
  before `main`, SIGXFSZ ignored, inherited descriptors marked close-on-exec)
  and `beadroll_git::runner` (`setsid` and SIGXFSZ's default between fork and
  exec), the two in bead; and xtask's `timing::heap`, the system allocator with
  a count of what it hands out, since `GlobalAlloc` is an unsafe trait.
- **Layout.** No `mod.rs` files (`clippy::mod_module_files`). A source file
  warns above 500 lines; split it long before 800. Each crate's `AGENTS.md` has
  the sections Purpose, Key entrypoints, Edit here for X, Invariants and Common
  commands.
- **Few dependencies, each for a reason.** No serde or serde_json in the
  product (beadroll-core reads and writes JSON itself: `docs/REVIEWING.md`
  §2); no tokio, since nothing is async (a consumer on tokio calls through
  `spawn_blocking`); no chrono or jiff (event times are UUIDv7 milliseconds,
  turned into dates by a small function); no `regex` (hand-written matchers,
  and `regex-lite` for git's messages); no anyhow (every error carries a code);
  no `wait-timeout` or `ctrlc` (a waiter thread is enough). `cargo deny`
  checks licences, advisories and sources (`deny.toml`).

## Variables the tests and drivers read

bead's own variables are in the README's *Configuration*. These are read only
by the tests, xtask or `infra/`:

| Variable | Read by | Meaning |
|---|---|---|
| `BEAD_UNDER_TEST` | tests, `infra/` | The `bead` to run, by its absolute path, instead of the one Cargo built (tests) or the installed `~/.local/bin/bead` (`infra/`) |
| `BEAD_FAILPOINTS` | bead | Faults to inject: `point=action`, `point@nth(k)=action` or `point@always=action`, separated by `;` (`crates/beadroll-store/src/failpoint.rs`) |
| `BEAD_TEST_CRASH_BEFORE_PUSH`, `BEAD_TEST_CRASH_AFTER_PUSH` | bead | The crash hooks: exit 98 before, 99 after an ownership command's publish; aliases of two failpoints |
| `BEAD_SWEEP`, `BEAD_SWEEP_GC`, `BEAD_SWEEP_CONCURRENT` | the sweep | `full` runs all 660 schedules; with `BEAD_SWEEP_GC=1` the remote collects garbage after every push it accepts; with `BEAD_SWEEP_CONCURRENT=1` each schedule's two follow-ups run at once |
| `BEAD_TEST_REMOTE`, `BEAD_DRIVE_HOSTS`, `BEAD_DRIVE_BEADS`, `BEAD_EXPECT_VERSION`, `BEAD_TEST_LOCAL_REPO` | `infra/` | Each script's docstring says what it needs |

## Reporting a problem

Open an issue with the command, its `--json` output (which carries the error
code), `bead info --json`, and the git version. If a journal is involved,
`bead info` says where the store is; the `events/` files are plain JSONL and
safe to attach once you have checked them for anything private.

## Licence

By contributing you agree that your contribution is licensed under the
project's terms: MIT or Apache-2.0, at the user's option.
