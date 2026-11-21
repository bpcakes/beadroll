# Working on beadroll

beadroll is a Beads-compatible issue tracker for coding agents working across git worktrees and
machines. The implementation is a Cargo workspace (Rust 1.100, pinned by `rust-toolchain.toml`):
`cargo build --package beadroll` builds the working copy as `target/debug/bead`, and `cargo install
--locked --force --path crates/beadroll --root ~/.local`, run from the checkout's root, installs it
as the `bead` command. `README.md` says what it does and why, and `docs/GUARANTEES.md` every
guarantee it keeps; `CONTRIBUTING.md` has the rules every crate keeps. `docs/REVIEWING.md` is
the map of the design, format 1 byte for byte, the decisions that are not findings, and the
rules reviewers work under. `docs/FINDINGS.md` is the
ledger of every review finding and how it was closed; `docs/reviews/` holds the reports verbatim.

## Layout

- `crates/` — six crates that depend only downwards, which the compiler enforces:
  `beadroll-core` (strict JSON and text, event validation, journals, replay into state, the
  dependency graph and readiness, views, Beads import planning and export; no I/O, processes,
  clock or environment), `beadroll-git` (the only code that starts git: the runner, its
  environments, remote URLs, push outcomes), `beadroll-store` (this machine: the store and its
  files, the workspace a checkout resolves to, identity, failpoints), `beadroll-sync` (local
  transactions, the link to the coordination remote, the ownership protocol) and `beadroll` (the
  `bead` binary: `commands`, `cli`); and `beadroll-test-support`, the sandbox and machines the
  black-box tests run `bead` on. Each has its own `AGENTS.md`. `Cargo.toml`, `Cargo.lock`,
  `rust-toolchain.toml`, `clippy.toml`, `deny.toml` and `.config/nextest.toml` are at the root.
- Tests — each crate's unit tests beside its code, and one integration-test binary per crate
  (beadroll-test-support, a test library, has none), `crates/<crate>/tests/<name>/main.rs`,
  organized by subject:
  `replay`, `storage`, `compat`, `commands`, `protocol`, `hardening`, and the slow `stress` and
  `sweep`. `crates/beadroll/tests/golden/` holds fixed reference outputs, and
  `crates/beadroll/tests/cli/parity/` checks compatibility across machines and versions.
- `xtask/` — `cargo xtask`: `check` runs the baseline, `coverage` reports which lines the suites
  run (cargo-llvm-cov; the nightly workflow keeps its report), `timing` records how long a command
  took, `bead` runs bead in a process that counts its heap; its tests keep the repository's hygiene
  and the CI matrix.
- `fuzz/` — cargo-fuzz targets for the segment reader and the Beads importer, run
  nightly: a workspace of its own on the nightly its `rust-toolchain.toml` pins, outside the main
  workspace; `fuzz/README.md` says how to seed and run them.
- `infra/` — drives against real machines and a throwaway GitHub repository, in stdlib Python;
  each script's docstring says what it needs. `selected.py` chooses the bead a driver runs here
  and records it before any test data is written; the `test_*.py` beside them test the drivers and
  their shared modules.
- `skills/` — what agents load rather than build. `skills/board` is the prototype of a message
  board for people and agents across projects, stdlib Python over `bead`: `scripts/board` (what
  each command prints; its docstring says what it needs), `scripts/messages.py` (the board
  through bead), `scripts/homes.py` (the homes Claude Code and Codex have on a machine, and what
  `board setup` puts in each) and `SKILL.md`, which sends an agent to `board prime`. It is here
  to be tried before `bead` learns the same commands, and goes when it has. No step of the
  baseline runs it; `python3 -B -m unittest discover -s skills/board/scripts` runs its tests.
- `check.sh` — the baseline: `cargo xtask check` with the options given.

## Before you change anything

- Run the baseline: `./check.sh` (about 4 minutes on 16 cores; `--stress` adds the randomised
  models, `--real` the real-export round trip). It is `cargo xtask check`: the workspace's tests
  through cargo-nextest (the protocol sweep's slice among them), the doctests, and the `infra/`
  drivers' tests, each step with a scratch `BEAD_HOME`. One crate: `cargo nextest run -p
  beadroll`; some tests by a filter: `cargo nextest run -E 'test(/^protocol::/)'`. The lints:
  `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features --locked
  -- -D warnings -D clippy::mod_module_files`, the docs: `RUSTDOCFLAGS='-D warnings' cargo doc
  --workspace --no-deps --locked`, and for `fuzz/`, from the root, `rustfmt --check
  --edition 2024 fuzz/src/lib.rs fuzz/fuzz_targets/*.rs` and `cargo clippy --manifest-path
  fuzz/Cargo.toml --all-targets --locked -- -D warnings`. CI (`.github/workflows/rust.yml`) runs
  them, the tests on Linux and macOS, cargo-deny, the `infra/` tests and `fuzz/`'s cargo-deny on
  every push and pull request, and `.github/workflows/rust-nightly.yml` the tests under git 2.40,
  the oldest supported, the coverage report, the stress models with the full sweep, `xtask`'s
  growth guards timed in release, and the fuzz targets for a time budget each.
- The default store `~/.local/state/beadroll` holds this repository's own real tracker. The tests
  give every machine a scratch `BEAD_HOME`; set one yourself for any manual experiment
  (`export BEAD_HOME=$(mktemp -d)`), and never run `bead`, `br` or `bv` inside other repositories
  on this machine: the origin URL would route you to a real workspace.
- The only real remote test data may go to is the throwaway repository the maintainer provides in
  `$BEAD_TEST_REMOTE` (the `infra/` scripts read it), and never its `main`. Do not change SSH, git
  or shell configuration on any machine.

## Making a change

- Rust code follows *Rules every crate keeps* in `CONTRIBUTING.md` and its crate's `AGENTS.md`.
  Crates depend only downwards (`beadroll-core` and `beadroll-git` → `beadroll-store` →
  `beadroll-sync` → `beadroll`); a helper two crates need moves down, never up, and only the
  `beadroll` crate's `cli` reads the environment and the working directory, and makes the clock
  (`Settings`' `Clock`, whose `real_ns` is bead's one reading of the wall clock). Match the
  surrounding style: comments in prose that say why, names that say what a function returns,
  errors whose codes a user can act on (`bead prime` lists them). Unknown input is reported,
  never guessed at; nothing is ever half-written. No `mod.rs` files, and a source file stays
  under 500 lines where it can and is split before 800, which xtask's hygiene tests enforce.
- Every behaviour change comes with a test in the module of its subject. A black-box test runs
  `bead` through `beadroll-test-support` (`Sandbox`, `Machine`): the binary Cargo built, unless
  `BEAD_UNDER_TEST` names another. A fault is injected through a failpoint (`BEAD_FAILPOINTS`,
  listed in `crates/beadroll-store/src/failpoint.rs`), never by patching or by sleeping:
  `backoff@always=skip` skips the wait between attempts, and `pause-until(PATH)` holds bead at a
  point for the test to act (`beadroll_test_support::Gate`); such a test's machine is `rust_only`,
  this build's binary. In-crate tests pass the failpoint registry in `Settings`, never through the
  process environment. Some waits cannot be a failpoint, such as a hook touching a file or a push
  that bead gave up on still running at the remote. Those poll with `wait_until`, or with
  `Sandbox::await_remote_settled` for the push, and both fail the test at their deadline; a fixed
  sleep never stands in for them.
- A test module is named for its subject and a test for the contract it checks, with any finding id
  in its doc comment, never the review's name in either; a helper's doc says what the helper
  returns. A test that crashes an ownership command names the failpoint,
  `ownership.before-publish=exit(98)` or `ownership.after-publish-ok=exit(99)`: the crash hooks
  `BEAD_TEST_CRASH_BEFORE_PUSH` and `BEAD_TEST_CRASH_AFTER_PUSH` are aliases kept for the older
  tests that use them, not to copy.
- Record deliberate decisions in `docs/REVIEWING.md` §2, every guarantee in `docs/GUARANTEES.md`,
  and what a new user reads first in `README.md`; a review finding you close gets its row in
  `docs/FINDINGS.md`. Format 1 is frozen: additions only.
- Run the fast baseline and the lints (*Before you change anything*) before committing, the stress
  suite when the sync protocol changed. The
  commit message says what changed in behaviour, in prose.
- After a push, a machine upgrades by rerunning its install (`cargo install --locked --force
  --path crates/beadroll --root ~/.local` from its checkout's root, or the `--git` form in the
  README); the `infra/` scripts run the installed `~/.local/bin/bead` on each machine, this one
  included, unless `BEAD_UNDER_TEST` (here) or `BEAD_DRIVE_BEADS` (by host) name another by its
  absolute path, and `BEAD_EXPECT_VERSION` makes each refuse a bead that reports another version or
  is a script.

## Releasing

1. `CHANGELOG.md`: move *Unreleased* into a dated version section; bump `[workspace.package]
   version` in `Cargo.toml` to match (`cargo build` then updates `Cargo.lock`), and run `cargo
   update --workspace --manifest-path fuzz/Cargo.toml` for `fuzz/Cargo.lock`, which CI reads
   `--locked` too.
2. `./check.sh --stress` green, and CI green on `main`.
3. `git tag -a vX.Y.Z -m "beadroll X.Y.Z"` and `git push origin main vX.Y.Z`. Nothing is
   published: machines build from source.
4. Each machine reinstalls (*Install* in the README) and checks `bead --version`.

## Work tracking (bead)

This repository tracks its work with `bead` (beadroll): one tracker shared by every worktree,
clone and machine of this repository, stored outside the working tree, never on a code branch.

- Start every session with `bead prime` and follow it. It prints the current workflow, who you
  are and what you hold; it is the source of truth, not this file.
- Find work with `bead ready`, claim an issue before working on it, record progress and decisions
  with `bead comments add`, split follow-ups with `bead create "title" --parent ID`, and close with
  `bead close ID -r "what was done"`. Every write is published at once; when a command says
  "not yet published" (`--offline`, or the remote did not answer), run `bead sync` before you stop.
- A claim belongs to your agent thread and lasts until release, close or takeover. A new session
  continues its own earlier work with `bead takeover ID -r "same task, new session"`; never take
  over someone else's work without a reason you have verified.
- Add `--json` for machine-readable output. Errors are structured; `bead prime` lists the codes
  and what to do about each.
- If a `.beads/` directory is still here, it is history: do not edit it or run `br`/`bd` in this
  repository. `bead export` writes Beads JSONL when another tool needs it.

<!-- Project specifics go here: the issue prefix and the epics to start from, what "done" means
     (tests, docs, merged where), and the branch or PR conventions a close reason should name. -->

Project specifics: the prefix is `roll-`, and most of the backlog hangs off the open epics, which
`bead list --type epic` lists. "Done" means the fast baseline and the lints are green, the docs
above are updated, and the change is on `main`. The block above is `bead onboard`'s output verbatim;
a test keeps it that way, so change the text in `ONBOARD` (`crates/beadroll/src/cli/texts.rs`).
