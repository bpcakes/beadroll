# Contributing

Thanks for looking. beadroll is small and deliberately boring: one package, the
standard library, git. The rules below are the ones that keep it that way.

## Setup

```sh
git clone https://github.com/bpcakes/beadroll && cd beadroll
./bead --version          # the launcher runs the working copy; nothing to install
./check.sh                # the baseline: every fast test module, about 20 minutes
```

Python 3.11+ and git 2.40+. No dependencies, so no virtual environment is
needed for development; `uv tool install --force .` installs the working copy
as the `bead` command if you want it on your PATH.

## The one rule about stores

The tests build their own repositories, bare remotes and machine homes in a
temporary directory and set a scratch `BEAD_HOME`; they never touch the default
store under `~/.local/state/beadroll`. **Do the same by hand**: before running
`bead` to try something, `export BEAD_HOME=$(mktemp -d)`, and run it only inside
a repository you made for the experiment. In any other checkout the origin URL
routes you to that repository's real tracker. Never push test data anywhere but
a throwaway remote of your own.

## Layout

- `beadroll/` — seven modules that import only downwards: `base` → `machine` →
  `model` → `queries` → `sync` → `commands` → `cli`. A helper two modules need
  moves down, never up. `AGENTS.md` describes each module.
- `tests/` — `unittest`, one module per subject (`test_replay`, `test_storage`,
  `test_compat`, `test_commands`, `test_protocol`, `test_hardening`; the slow
  `test_stress` and `test_sweep`; `test_hygiene` and `test_check`). `tests/support.py` has the
  sandbox and the `Machine` helper.
- `README.md` is what the tool does; `docs/REVIEWING.md` is the map of the design
  and the list of deliberate decisions; `docs/FINDINGS.md` is the ledger of every
  review finding and how it was closed.

## Making a change

1. Every behaviour change comes with a test in the module of its subject.
   In-process tests replace a name with `support.patched(name, value)`, which
   reaches every module that imported it; to skip the wait between sync
   attempts patch `pause`, never the global `time.sleep`.
2. Match the surrounding style: comments in prose that say why; names that
   say what a function returns; errors are `BeadError(code, message)` with a
   code a user or an agent can act on (`bead prime` lists them). Unknown input
   is reported, never guessed at; nothing is ever half-written.
3. Format 1 is frozen: a change to what events or the published manifest
   contain must be additive, so that an older bead sets the new thing aside
   rather than misreads it (see *Format and upgrades* in the README).
4. A deliberate decision goes in `docs/REVIEWING.md` §2; user-facing behaviour in
   `README.md`. Keep the two agent texts (`bead prime`, `bead onboard`) true:
   `AGENTS.md` carries the onboard block verbatim and a test checks that.
5. Run `./check.sh` before opening a pull request; CI runs it on Linux and
   macOS. Run `./check.sh --stress` when you touched the sync protocol.
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
