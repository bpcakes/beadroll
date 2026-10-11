# Changelog

All notable changes to beadroll. The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow [Semantic Versioning](https://semver.org/). The on-disk and published format is
versioned separately (format 1, frozen; see *Format and upgrades* in the README).

## [Unreleased]

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
