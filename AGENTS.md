# Working on beadroll

beadroll is a Beads-compatible issue tracker for coding agents working across git worktrees and
machines. The implementation is one module, `beadroll.py` (Python 3.12+, standard library only);
`./bead` links to it, and `uv tool install .` installs it as the `bead` command. `README.md` says
what it does and why. `REVIEWING.md` is the map of the design, the decisions that are not
findings, and the rules reviewers work under. `FINDINGS.md` is the ledger of every review finding
and how it was closed; `reviews/` holds the reports verbatim.

## Layout

- `beadroll.py` — the implementation, in sections: locations, identity, journal, validation,
  replay, writes, queries, sync, commands, cli. `pyproject.toml` packages it; nothing else is code.
- `tests/` — `unittest` modules by subject: `test_replay`, `test_storage`, `test_compat`,
  `test_commands`, `test_protocol`, `test_hardening`, and the slow `test_stress`, `test_sweep`.
  `tests/support.py` has the `Sandbox`/`Machine` helpers (a bare remote and machines with their
  own homes); `tests/__init__.py` sets a scratch `BEAD_HOME`.
- `infra/` — drives against real machines and a throwaway GitHub repository; each script's
  docstring says what it needs. `check.sh` is the baseline.

## Before you change anything

- Run the baseline: `./check.sh` (about 15 minutes; `--stress` adds the randomised models and the
  protocol sweep, `--real` the real-export round trip). One module:
  `python3 -m unittest tests.test_commands`; one class or test by its dotted name.
- The default store `~/.local/state/beadroll` holds this repository's own real tracker. The test
  modules set a scratch `BEAD_HOME`; set one yourself for any manual experiment
  (`export BEAD_HOME=$(mktemp -d)`), and never run `bead`, `br` or `bv` inside other repositories
  on this machine: the origin URL would route you to a real workspace.
- The only real remote test data may go to is `$BEAD_TEST_REMOTE`,
  and never its `main`. Do not change SSH, git or shell configuration on any machine.

## Making a change

- Keep `beadroll.py` one stdlib-only module. Match the surrounding style: comments in prose that
  say why, names that say what a function returns, `BpError(code, message)` with codes a user can
  act on (`bead prime` lists them). Unknown input is reported, never guessed at; nothing is ever
  half-written.
- Every behaviour change comes with a test in the module of its subject. Tests that skip the wait
  between attempts patch `pause`, never the global `time.sleep`.
- Record deliberate decisions in `REVIEWING.md` §2 and user-facing behaviour in `README.md`; a
  review finding you close gets its row in `FINDINGS.md`. Format 1 is frozen: additions only.
- Run the fast baseline before committing, the stress suite when the sync protocol changed. The
  commit message says what changed in behaviour, in prose.
- After a push, a machine upgrades by rerunning its install (`uv tool install --force ...`); the
  `infra/` scripts expect `scp beadroll.py host:beads-poc-test/bead`.

## Work tracking (bead)

This repository tracks its work with `bead` (beadroll): one tracker shared by every worktree,
clone and machine of this repository, stored outside the working tree, never on a code branch.

- Start every session with `bead prime` and follow it. It prints the current workflow, who you
  are and what you hold; it is the source of truth, not this file.
- Find work with `bead ready`, claim an issue before working on it, record progress and decisions
  with `bead comments add`, split follow-ups with `bead create "title" --parent ID`, close with
  `bead close ID -r "what was done"`, and `bead sync` before you stop.
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
