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
  is touched and nothing is committed. That is the path to take. Only if you
  cannot push to origin, or want the tracker elsewhere, use a tracked
  `.beadroll.json` (*Configuration* in the README: `bead init --tracked
  --remote …`), and commit it on the default branch in the same change.
- A current `.beads/issues.jsonl`. If your tracker lives in `br`'s or `bd`'s
  database only, or the database has changes the file lacks, export it first
  (`br sync --flush-only`, or `bd export`).

## A dry run first

```sh
cd the-repo
bead init --import .beads/issues.jsonl --dry-run
```

It reads and checks the file exactly as the import would, and writes nothing,
starts no store and asks no remote. It says:

- how many issues would come in, by status, and how many the tracker already
  has (an import leaves those as they are);
- every record the import would be refused for, not only the first: one bad
  record refuses the whole file, so fix them all before the real run;
- the `in_progress` issues, which arrive with no owner (below);
- dependencies on keys the file does not hold;
- the branches, local or remote-tracking, with commits touching
  `.beads/issues.jsonl` that this checkout's HEAD does not have, and the other
  worktrees with changes to it not committed. Beads keeps its tracker on every
  branch, so work recorded on one of them is not in this checkout's copy. Bring
  it here first (merge the branch, or commit and merge the worktree's changes,
  then export again): a later import adds issues but never another copy's newer
  closes or edits.

`--json` says the same for an agent; `importable` is whether the import would
go through. `bead import FILE --dry-run` does the same against a tracker that
already exists.

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

Do it once, on one machine. Every other checkout and machine runs `bead sync`
to fetch the tracker; an `init` there finds it published and joins it rather
than starting a second one.

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
