# Changelog

All notable changes to beadroll. The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow [Semantic Versioning](https://semver.org/). The on-disk and published format is
versioned separately (format 1, frozen; see *Format and upgrades* in the README).

## [Unreleased]

One golden path for setting up a repository's tracker (roll-dih8.13): `bead init` once, from any
checkout, nothing committed; `bead sync` on every other checkout and machine.

- `bead init` asks the remote before it starts a tracker. A tracker the repository already
  publishes there is joined, with no second `init` event, and adopted as `sync` adopts it when it
  was published under another id (a moved repository, or a tracked `.beadroll.json` the clone
  lacks); a failure to join it fails `init`, which then starts nothing. `init --tracked` names
  the tracker the checkout has, joined just now or here already (a tracked id the remote
  publishes, one an earlier `init` adopted), minting a new id only where there is none, and
  refuses (`CONFIG`) to pin a URL-derived one, whose id would stop matching when the URL changed. A published tracker it cannot adopt is refused with
  `WORKSPACE_MISMATCH`. Before, `init` there started a second tracker, whose writes could never be
  published.
- `NOT_INITIALIZED` says to run `bead sync` first and `bead init` only to start a tracker the
  repository does not have; when the remote publishes none, `sync` says how to bring a moved
  repository's tracker along to that remote (`BEAD_WORKSPACE=ID bead sync --remote origin`, the
  remote as the sync chose it), naming this machine's tracker of the same name when it has one.
- `bead info` (`tracked_config_warning`) and `bead prime` warn while `.beadroll.json` is not
  committed, differs from its commit as JSON (even under `--skip-worktree`; line endings git
  converts are no difference), or is not on origin's default branch, also when the tree lacks the
  file and HEAD's copy is the one in use: the other checkouts and machines do not see it. A checkout with no
  `refs/remotes/origin/HEAD` is told it cannot check that (`git remote set-head origin --auto`).
- A coordination remote is kept as it was chosen: a configured remote's name (`--remote`,
  `$BEAD_REMOTE`, `.beadroll.json`; origin when none is chosen), or a URL or a path as given. A
  store remembers the name, which every command looks up again, so `git remote set-url` is
  followed for any remote as it always was for origin; it used to remember the URL the name led
  to then. A name means the URL it is configured with for `git push` (its first push URL, else its
  first URL), as origin itself did; it used to be the fetch URL as git expanded it, another place
  when the two differ. bead gives git that URL as a remote defined for the command, never as an
  argument git could take for another remote's name, so that git rewrites it as for `git fetch
  NAME` and `git push NAME` (a remote's own push URL without `pushInsteadOf`), from the top of the
  checkout; origin too, by the spelling it is configured with, which `info` and `sync` now show,
  where bead used to make a relative path absolute that a rule could then no longer rewrite. A
  tracked remote is the store's when it is the same name or the same URL, or, in a
  store an older bead left with a tracked name saved as git expanded it, where git's own answers
  agree; any other change is refused (`CONFIG`) until made deliberately. Origin's own URL given as
  a URL spelled as configured is origin, reached as origin is, its own push URL included, and
  `init --tracked --remote` stores it as `origin`, so the file follows origin when the repository moves. A relative path
  typed in a subdirectory is read from there, also by `init --tracked`, which stores it from the
  top of the checkout whether it exists or not; a path a rule rewrites is left to git as written.
  Going back to 0.2.0, a store whose tracked and remembered remote are one name is refused there
  until `bead sync --remote NAME`.
- `bead init --json` reports `unpublished` for the tracker it wrote to, also when that is not the
  checkout's own (a joined, adopted or tracked one).
- The README describes the golden path and moving a repository, and MIGRATING runs the import once.
- Concurrent bead processes of one store (agents in several worktrees) no longer fail a command
  with `REMOTE_UNREACHABLE` when one fetch finds another's fetch ref gone while git reads the refs
  (`fatal: bad object refs/beads/fetch/…`), or the directory of its own removed while git makes
  it: the fetch is tried again after a short wait, five attempts in all (roll-dih8.12).
- The protocol sweep fails a schedule whose fault never fired: the operation must end at its
  boundary (exit 98 or 99, a SIGKILL, or the remote's stalled push). The sync schedules record
  their local work offline, so the 48 at the push and timeout boundaries now push, where they
  passed with nothing to publish, and the default slice stops a sync at its push (roll-dih8.10).
- A publishing write (a sync, a claim, a published local write) hashes only the journal files that
  changed since bead last hashed them, once: the active segment, new attempt files and files whose
  length or identity changed, where it hashed every segment and attempt file of the store twice
  (roll-lqjj.1). It no longer reads every segment to count its lines or to compare it with the
  published copy, and checks a segment for a torn tail by its last byte. What it remembers of each
  file (its identity on disk, its lines, its blob) is a cache, `sync.git/journal-memo`: a file
  that keeps its identity is not hashed again, and its remembered blob goes into a tree only when
  it is sure to be in `sync.git`; the tree published is the one hashing every file makes.
- `cargo xtask timing --sync N [--ownership] [--rotate BYTES]` times a publishing `bead sync` on a
  scratch store of N events against a local bare remote, with the store's memo of its journal files
  and without it; `docs/RUST-PORT.md` Appendix A has the figures at 200,000 events.

## [0.2.0] — 2026-10-11

bead is now the Rust implementation: one binary, built from the Cargo workspace in `crates/`,
where 0.1.0 was a Python package. It reads and writes the same stores, journals, receipts and
published trees, so machines can be upgraded in any order and a store can go back to the last
Python bead, tagged `python-0.1.0`; and it keeps the commands, their flags, output, error codes and
exit statuses, help pages included. It differs only where Python's bead failed: where Python died
with a traceback (an unparsable setting such as `BEAD_NET_TIMEOUT`, a damaged store file) it
answers with a structured error (`docs/REVIEWING.md` §2), and on Linux under a locale that is not
UTF-8 it names a coordination ref by its UTF-8 bytes, as `.beadroll.json` holds it, where Python
used the locale's codeset (`roll-k2wy.18`). Each of the
Python suite's 470 tests was ported to Rust, the black-box ones passing against both
implementations, and fleets that mixed the two converged.

It is installed from source, with rustup: from a checkout's root, `cargo install --locked --force
--path crates/beadroll --root ~/.local`; without one, `cargo +1.99.0 install --locked --force
--config net.git-fetch-with-cli=true --git ssh://git@github.com/bpcakes/beadroll.git beadroll
--root ~/.local`. Either puts `bead` in `~/.local/bin` and upgrades it when run again; `uv tool
install` and the Python package are gone. It is faster: it starts in about 2 ms where Python took
about 40, replays 200,000 events in a quarter of a second, and a publishing write starts 14 gits
instead of 26 or 27.

Changes since 0.1.0, made before the cutover and carried over by it unchanged:

- Every bead reads a journal line the same way, whatever Python it runs on: as UTF-8 (a
  byte-order mark, UTF-16 or UTF-32 is no longer guessed from its first bytes), at most 512 levels
  deep, with NaN, Infinity and numbers too large for a float refused; ids and dependencies match
  the UUID shape whole, and the published manifest's format must be the integer 1. Key numbers,
  segment names and URL schemes are ASCII: a renamed copy of `x.٣` keeps its own name. Events
  that made replay or a query fail (or answer in a Python version's own words) are refused by
  name and reported under `integrity`, and an import fails whole on a record whose `created_at` is
  null or whose text holds a lone surrogate (`docs/FINDINGS.md`, U01–U11).
- `bead ready ID` offers only the ready issues in ID's subtree, at any depth and through every
  parent link; `--epic ID` now means the same, as in br (it used to match ID's direct children
  only, which `--parent ID` still does). A scope that names no issue is `ISSUE_NOT_FOUND`, in
  `list --parent` too, instead of an empty answer, and two scopes at once (a repeated `--parent`
  or `--epic` included) are a `USAGE` error.
- Verification fails on every selected protocol-sweep failure, including the previously
  exempt G12 scenarios; the default slice covers interrupted owner close/delete retries.
- `check.sh --real` reports failure when the round-trip driver fails, a bead command fails,
  or exported records differ, and preserves the full failing report. Imports stay offline
  in the scratch store.

## [0.1.0] — 2026-10-09

First release.

- One tracker per repository, shared by every worktree, clone and machine through one git ref
  (`refs/beads/journal`) on the repository's own remote; nothing on code branches.
- Append-only per-machine journals of causally ordered events (UUIDv7 ids), replayed into the
  current state; concurrent edits are kept and flagged, never merged by a clock.
- Claims settled online by a conditional update of the ref, persistent until release, close or
  takeover; every write published before the command returns, `--offline` to keep it local, and
  every command reporting what the remote still lacks.
- Beads JSONL import and export, checked against `br` on real trackers; custom statuses kept
  verbatim.
- `bead prime` and `bead onboard`: the workflow and the instruction block an agent needs.
- A private `sync.git` that bead maintains itself and rebuilds if damaged.
- Seven independent reviews closed; the ledger is `docs/FINDINGS.md`.

[Unreleased]: https://github.com/bpcakes/beadroll/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/bpcakes/beadroll/releases/tag/v0.2.0
[0.1.0]: https://github.com/bpcakes/beadroll/releases/tag/v0.1.0
