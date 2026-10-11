# Contributing

Thanks for looking. beadroll is small and deliberately boring: one binary, a
few dependencies, git. The rules below are the ones that keep it that way. It
was a Python package until 0.2.0 (the tag `python-0.1.0`); `docs/RUST-PORT.md`
is the plan its port to Rust followed.

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
- `xtask/` — `cargo xtask check` (the baseline) and `cargo xtask timing`.
- `infra/` — stdlib-Python drivers against real machines, with their tests.
- `README.md` is what the tool does; `docs/REVIEWING.md` is the map of the design
  and the list of deliberate decisions; `docs/FINDINGS.md` is the ledger of every
  review finding and how it was closed.

## Making a change

1. Every behaviour change comes with a test in the module of its subject. A
   fault is injected through a failpoint (`BEAD_FAILPOINTS`,
   `docs/RUST-PORT.md` §4.5), never by patching: to skip the wait between sync
   attempts, `backoff@always=skip`, never a shortened sleep. Black-box tests run
   `bead` through `beadroll-test-support`'s `Sandbox` and `Machine`.
2. Match the surrounding style: comments in prose that say why; names that
   say what a function returns; errors carry a code a user or an agent can
   act on (`bead prime` lists them). Unknown input is reported, never guessed
   at; nothing is ever half-written. `docs/RUST-PORT.md` §2.2 has the rules
   every crate keeps (no panics outside tests, no `mod.rs` files, no output
   but through the CLI's output module).
3. Format 1 is frozen: a change to what events or the published manifest
   contain must be additive, so that an older bead sets the new thing aside
   rather than misreads it (see *Format and upgrades* in the README).
4. A deliberate decision goes in `docs/REVIEWING.md` §2; user-facing behaviour in
   `README.md`. Keep the two agent texts (`bead prime`, `bead onboard`) true:
   `AGENTS.md` carries the onboard block verbatim and a test checks that.
5. Run `./check.sh` before opening a pull request; CI runs the tests on Linux
   and macOS, with the lints of `.github/workflows/rust.yml` (`AGENTS.md` lists
   them). Run `./check.sh --stress` when you touched the sync protocol.
6. Commit messages say what changed in behaviour, in prose; the subject line
   is the change, not the file.

## Reporting a problem

Open an issue with the command, its `--json` output (which carries the error
code), `bead info --json`, and the git version. If a journal is involved,
`bead info` says where the store is; the `events/` files are plain JSONL and
safe to attach once you have checked them for anything private.

## Licence

By contributing you agree that your contribution is licensed under the
project's terms: MIT or Apache-2.0, at the user's option.
