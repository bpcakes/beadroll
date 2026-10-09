# Changelog

All notable changes to beadroll. The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow [Semantic Versioning](https://semver.org/). The on-disk and published format is
versioned separately (format 1, frozen; see *Format and upgrades* in the README).

## [Unreleased]

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

[Unreleased]: https://github.com/bpcakes/beadroll/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/bpcakes/beadroll/releases/tag/v0.1.0
