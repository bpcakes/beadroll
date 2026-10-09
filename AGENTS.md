# Working on beadroll

beadroll is a Beads-compatible issue tracker for coding agents working across git worktrees and
machines. The implementation is the package `beadroll/` (Python 3.11+, standard library only);
`./bead` runs the working copy, and `uv tool install .` installs it as the `bead` command. Its Rust
port (`docs/RUST-PORT.md`) grows beside it in a Cargo workspace until the cutover. `README.md` says
what it does and why. `docs/REVIEWING.md` is the map of the design, the decisions that are not
findings, and the rules reviewers work under. `docs/FINDINGS.md` is the ledger of every review finding
and how it was closed; `docs/reviews/` holds the reports verbatim.

## Layout

- `beadroll/` — the implementation, seven modules that import only downwards: `base` (errors,
  strict JSON and text, the git runner, remote URLs), `machine` (the store and its files, the
  workspace a checkout resolves to, identity), `model` (event validation, journals, replay into
  state), `queries` (the dependency graph, readiness, presentation, Beads export), `sync` (local
  transactions, the link to the coordination remote, the ownership protocol), `commands`, `cli`.
  `__init__.py` re-exports every name as one namespace for tests and scripts. `pyproject.toml`
  packages it.
- `tests/` — `unittest` modules by subject: `test_hygiene`, `test_check`, `test_replay`,
  `test_storage`, `test_compat`, `test_commands`, `test_protocol`, `test_hardening`, and the slow
  `test_stress`, `test_sweep`.
  `tests/support.py` has the `Sandbox`/`Machine` helpers (a bare remote and machines with their
  own homes); `tests/__init__.py` sets a scratch `BEAD_HOME`.
- `infra/` — drives against real machines and a throwaway GitHub repository; each script's
  docstring says what it needs. `check.sh` is the baseline.
- `crates/` — the Rust port: `beadroll-core`, `beadroll-git`, `beadroll-store`, `beadroll-sync`,
  `beadroll` (the `bead` binary) and `beadroll-test-support` (the Rust `tests/support.py`), each
  with its own `AGENTS.md`; `Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml` are at the root.
- `xtask/` — `cargo xtask`: `oracle` runs this Python suite against any `bead` binary, without
  the tests `oracle-skip.txt` lists; `inventory` maps every Python test (`inventory.txt`) to the
  Rust tests that port it; `timing` records how long a command took.

## Before you change anything

- Run the baseline: `./check.sh` (about 20 minutes; `--stress` adds the randomised models and the
  protocol sweep, `--real` the real-export round trip). One module:
  `python3 -m unittest tests.test_commands`; one class or test by its dotted name. CI
  (`.github/workflows/check.yml`) runs the fast baseline on every push and pull request, Linux on
  every supported Python and macOS on the newest, and the stress suite nightly or on request.
- The Rust workspace: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --all-features --locked -- -D warnings -D clippy::mod_module_files`, `BEAD_UNDER_TEST=$PWD/bead
  cargo nextest run` (the ported black-box tests pass against Python's launcher until Phase 2d),
  and `cargo xtask oracle --bead ./bead` (this suite through `BEAD_UNDER_TEST`; it must pass
  completely). `.github/workflows/rust.yml` runs them on every push and pull request.
- The default store `~/.local/state/beadroll` holds this repository's own real tracker. The test
  modules set a scratch `BEAD_HOME`; set one yourself for any manual experiment
  (`export BEAD_HOME=$(mktemp -d)`), and never run `bead`, `br` or `bv` inside other repositories
  on this machine: the origin URL would route you to a real workspace.
- The only real remote test data may go to is the throwaway repository the maintainer provides in
  `$BEAD_TEST_REMOTE` (the `infra/` scripts read it), and never its `main`. Do not change SSH, git
  or shell configuration on any machine.

## Making a change

- Keep the package stdlib-only and its modules importing downwards only (`base` → `machine` →
  `model` → `queries` → `sync` → `commands` → `cli`); a helper two modules need moves down, never
  up. Match the
  surrounding style: comments in prose that
  say why, names that say what a function returns, `BeadError(code, message)` with codes a user can
  act on (`bead prime` lists them). Unknown input is reported, never guessed at; nothing is ever
  half-written.
- Every behaviour change comes with a test in the module of its subject. In-process tests replace a
  beadroll name with `support.patched(name, value)`, which reaches every module that imported it;
  to skip the wait between attempts patch `pause`, never the global `time.sleep`. A subprocess
  driver gets every name from `runpy.run_path(bead)` (the launcher re-exports the package) and
  replaces one by setting it on every `beadroll*` module in `sys.modules` (see `tests/test_sweep.py`).
- Rust code follows `docs/RUST-PORT.md` §2.2 and its crate's `AGENTS.md`. A Python test reaches bead
  only through `support.BEAD`, which `BEAD_UNDER_TEST` selects. Adding, renaming or removing a
  Python test updates `xtask/inventory.txt`, and `xtask/oracle-skip.txt` when the test cannot run
  against another binary: the inventory refuses a list that disagrees with the suite, and the
  oracle a skip list that names a test the suite does not have.
- Record deliberate decisions in `docs/REVIEWING.md` §2 and user-facing behaviour in `README.md`; a
  review finding you close gets its row in `docs/FINDINGS.md`. Format 1 is frozen: additions only.
- Run the fast baseline before committing, the stress suite when the sync protocol changed. The
  commit message says what changed in behaviour, in prose.
- After a push, a machine upgrades by rerunning its install (`uv tool install --force ...`, or a
  venv `pip install` where there is no uv); the `infra/` scripts run the installed `~/.local/bin/bead`
  on each machine and `./bead` here.

## Releasing

1. `CHANGELOG.md`: move *Unreleased* into a dated version section; bump `__version__` in
   `beadroll/base.py` to match.
2. `./check.sh --stress` green, and CI green on `main`.
3. `uv build` (wheel and sdist into `dist/`), `uv publish` with a PyPI token (`UV_PUBLISH_TOKEN`),
   then `git tag -a vX.Y.Z -m "beadroll X.Y.Z"` and `git push origin main vX.Y.Z`; a GitHub release
   with the changelog section as its notes.
4. Each machine upgrades with `uv tool install --force beadroll` (or its channel from the README).

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

Project specifics: the prefix is `roll-`; the backlog hangs off epic `roll-dih8`, and deferred
items (`bead list --status deferred`) wait for the open-source milestone. "Done" means the fast
baseline is green, the docs above are updated, and the change is on `main`. The block above is
`bead onboard`'s output verbatim; a test keeps it that way, so change the text in `beadroll.py`.
