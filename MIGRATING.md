# Migrating a tracker from Beads (`br` / `bd`) to bead, and back

The whole move is one command per repository plus two decisions. What follows
is the procedure as run on a real tracker (227 issues, 1.9 MB of JSONL): what
to do, what to check, and the two things that are not obvious.

## Before

- `bead` installed on this machine (`cargo install --locked --force --path
  crates/beadroll --root ~/.local` from a checkout's root; *Install* in the
  README). Nothing is needed on the other machines until they sync.
- The repository has an `origin` you can push to. bead derives the tracker's
  identity from that URL and publishes to `refs/beads/journal` there; no branch
  is touched. If you cannot push to origin, or want the tracker elsewhere, see
  *Configuration* in the README (`bead init --tracked --remote …`).
- A current `.beads/issues.jsonl`. If your tracker lives in `br`'s or `bd`'s
  database only, export it first (`br sync --export-only`, or `bd export`).

## The move

```sh
cd the-repo
bead init --import .beads/issues.jsonl
```

That imports every record — ids, parent-child keys, dependencies, comments,
labels, statuses, timestamps — and **publishes the tracker at once**: when the
command returns, `refs/beads/journal` on origin holds it. The output says how
many records came in and whether anything is still unpublished (only if the
remote did not answer; then `bead sync` later).

Check it:

```sh
bead info            # the workspace id, the remote, the count, integrity: []
bead export > /tmp/roundtrip.jsonl         # the same records back (field order follows br's; compare canonically)
bead ready; bead blocked                   # should match br ready / br blocked --limit 0
```

`bead export` writes the same records back, so `br` or `bv` run against a
scratch copy of it give the same answers; `infra/compat_real.py` is the check
the project runs on every real tracker it can find (import, canonical
round trip, `ready` and `blocked` against `br`, `bv` on the export).

## The two things that are not obvious

**1. Issues that were `in_progress` arrive without an owner.** In Beads that
status is a label; in bead it means a claim, and an imported record carries
none. Such issues are neither ready nor held by anyone until somebody acts:

```sh
bead list --status in_progress
bead claim ID                       # whoever is actually working on it
bead update ID --status open        # or put it back on the frontier
```

**2. `.beads/` is now history.** Nothing in bead reads or writes it. If agents
in this repository are told to run `br`, they will keep writing to the old
tracker. Replace the instruction block in `AGENTS.md` / `CLAUDE.md` with the
one bead prints:

```sh
bead onboard >> AGENTS.md      # then delete the br/bd lines and fill in the project specifics
```

and either delete `.beads/` or leave it as a frozen export for tools that
still read it (`bv` reads `bead export -o .beads/issues.jsonl` just as well).
`git rm` is fine: the data is in the journal, published, and exportable.

## The other machines

Nothing to migrate. In any clone or worktree of the same repository:

```sh
bead sync        # finds the tracker from the origin URL, pulls the journal
bead ready
```

From then on every write on any machine is published before the command
returns; `bead sync` is the catch-up after `--offline` or a remote that did
not answer.

## Several projects at once

The import is per repository and takes seconds; the instruction rewrite is the
part worth scripting. For each repository: `bead init --import
.beads/issues.jsonl`, swap the agent block, commit `AGENTS.md`. Do a few first:
the imported `in_progress` issues need a human decision each.

## Back to Beads

```sh
bead export -o .beads/issues.jsonl
br sync --import-only            # or bd's equivalent
```

The export carries everything that came in plus everything bead added, in the
same record shape. Comments made in bead export with integer ids from a range
Beads never mints (so a later re-export agrees); custom statuses, if any
arrived, go back out verbatim. Claims have no Beads equivalent and are not
exported; an issue held in bead is `in_progress` in the export.

Leave `refs/beads/journal` on origin: it costs nothing and lets you come back.
Delete it with `git push origin :refs/beads/journal` if you want it gone.
