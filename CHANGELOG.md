# Changelog

All notable changes to beadroll. The format follows [Keep a Changelog](https://keepachangelog.com/);
versions follow [Semantic Versioning](https://semver.org/). The on-disk and published format is
versioned separately (format 1, frozen; see *Format and upgrades* in the README).

## [Unreleased]

- Every bead reads a journal line the same way, whatever Python it runs on: as UTF-8 (a
  byte-order mark, UTF-16 or UTF-32 is no longer guessed from its first bytes), at most 512 levels
  deep, with NaN, Infinity and numbers too large for a float refused; ids and dependencies match
  the UUID shape whole, and the published manifest's format must be the integer 1. Key numbers,
  segment names and URL schemes are ASCII: a renamed copy of `x.٣` keeps its own name. Events
  that made replay or a query fail (or answer in a Python version's own words) are refused by
  name and reported under `integrity`, and an import fails whole on a record whose `created_at` is
  null or whose text holds a lone surrogate (`docs/FINDINGS.md`, U01–U11).
- The Rust port's core (`docs/RUST-PORT.md`, Phase 1): `beadroll-core`, the replay, queries,
  views, export and import planning of this implementation, proven equal to it by
  `cargo xtask diff` over the journals the test suite builds and generated histories, with the
  suite's tests of the core ported to Rust. Nothing changes for users.
- `bead ready ID` offers only the ready issues in ID's subtree, at any depth and through every
  parent link; `--epic ID` now means the same, as in br (it used to match ID's direct children
  only, which `--parent ID` still does). A scope that names no issue is `ISSUE_NOT_FOUND`, in
  `list --parent` too, instead of an empty answer, and two scopes at once (a repeated `--parent`
  or `--epic` included) are a `USAGE` error.
- Verification fails on every selected protocol-sweep failure, including the previously
  exempt G12 scenarios; the default slice covers interrupted owner close/delete retries.
- The Rust port's foundations (`docs/RUST-PORT.md`, Phase 0): a Cargo workspace beside the
  package, its CI, and `cargo xtask oracle`, which runs this test suite against any `bead`
  binary. The tests and the `infra/` drivers they run (`compat_real.py`, `roundtrip.py`) now
  run the bead `BEAD_UNDER_TEST` names, and Python bead refuses to start while
  `BEADROLL_ORACLE_REFUSALS` is set. Nothing else changes for users.
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
