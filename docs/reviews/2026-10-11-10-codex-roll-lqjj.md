# Review 10: Codex reviews of epic roll-lqjj

2026-10-11. `codex review` (model gpt-6-astra, reasoning effort xhigh, read-only sandbox) of each
task of epic `roll-lqjj` on branch `roll-lqjj-proportional-cost`, round by round: the commits of
one task since the commit it started from, with the epic's and the task's recorded decisions as its
instructions. Each round's review is kept as Codex printed it, its home directory written as
`/Users/<name>/`; `FINDINGS.md` (ids L) records what each finding became.

## roll-lqjj.1, round 1: `e0f380f6d5eb76301ea5c25537982a9cffc4550a..a614ca53edc8b8dfaef12d474e0c61ba456615c0`

Reviewed only e0f380f6d5eb76301ea5c25537982a9cffc4550a..a614ca53edc8b8dfaef12d474e0c61ba456615c0. The range introduces scratch-repository isolation and missing-object recovery regressions. The baseline could not run because the read-only sandbox prevented Cargo from opening its build lock.

Full review comments:

- [P1] Clear inherited Git variables in the timing setup helper — /Users/<name>/Documents/beadroll/xtask/src/timing/sync.rs:225-230
  When `timing --sync` inherits an exported `GIT_DIR`, `current_dir(dir)` does not isolate these commands to the scratch repository. `World::new` uses this helper for repository initialization, remote configuration and committing, so setup can modify the caller's real repository or fail against its configuration. The sanitization in `World::bead` happens too late. Clear inherited Git variables for setup commands too, as required by [xtask/AGENTS.md:50–52](xtask/AGENTS.md#L50-L52).

- [P2] Recover when a memoized published blob is missing locally — /Users/<name>/Documents/beadroll/crates/beadroll-sync/src/link/write.rs:46-50
  If a published journal blob disappears from `sync.git` while its tree and memo survive, this branch still reuses its ID. `mktree` then reports `entry ... object ... is unavailable`, which `local_damage()` does not classify as rebuildable, leaving the command failing with `GIT_ERROR` and the unusable memo intact. Previously, full hashing recreated the missing blob from the journal. Add an unavailable-object fallback that invalidates and rehashes the cached entry or rebuilds the repository, preserving the [cache recovery contract](docs/REVIEWING.md#L212-L215).
