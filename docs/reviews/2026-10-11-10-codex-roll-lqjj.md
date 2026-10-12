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

## roll-lqjj.1, round 2: `e0f380f6d5eb76301ea5c25537982a9cffc4550a..b1b1e7b35d0c7688979eb084da87d6cbca6fc329`

Reviewed only e0f380f6d5eb76301ea5c25537982a9cffc4550a..b1b1e7b35d0c7688979eb084da87d6cbca6fc329. No actionable defects found. Tests remain unverified: ./check.sh could not start because the read-only sandbox denied access to Cargo’s build lock.

## roll-dih8.11, round 1: `cfe278a93d326abdcd1ddebee287871c30a94c6b..ef7fec6efdccca4ed0a4fb84889d1f68fd7c312e`

Reviewed cfe278a93d326abdcd1ddebee287871c30a94c6b..ef7fec6efdccca4ed0a4fb84889d1f68fd7c312e (HEAD); no actionable defects found. Prebuilt binaries passed 117 core tests and five targeted xtask tests. A fresh build and full baseline were not run because the sandbox is read-only.

## roll-lqjj.2, round 1: `1527ea8ff4dc47e7d3d51954c2d1bef892b5d468..01e4c2ebf31392a2bfb1661918bfb4e01aeda880`

Reviewed exactly 1527ea8ff4dc47e7d3d51954c2d1bef892b5d468..01e4c2ebf31392a2bfb1661918bfb4e01aeda880 (HEAD). Source tracing identifies three ways cached results can diverge from replay. All 52 prebuilt core unit tests passed; the full baseline and new reproductions were not run in the read-only sandbox.

Full review comments:

- [P2] Register batch positions before processing events causally — /Users/<name>/Documents/beadroll/crates/beadroll-core/src/state/seen/compact.rs:46-48
  When appended records from one machine are `[B, A]`, both depend on the cached frontier, and `A.id < B.id`, causal processing visits A first. Its predecessor B has not entered `applicable` yet, so A passes validation; B subsequently passes too. Full replay pre-registers both positions and rejects A for not observing B. The projection therefore applies an event replay rejects and hides the chain break from `Link::merge`. Populate all batch positions before observation, or decline this ordering, preserving the [projection/replay invariant](crates/beadroll-core/AGENTS.md#L69-L70).

- [P2] Verify that growth preserves the cached file prefix — /Users/<name>/Documents/beadroll/crates/beadroll-store/src/projection.rs:174-177
  An in-place edit to an existing line followed by an append passes this check despite changing the cached prefix. For example, change one issue's title without changing its byte count, then append a frontier-based event for another issue. Catch-up reads only the appended bytes, retains the old title, and saves that incorrect state under the file's new identity. Deleting the projection then changes the answer. Verify the prefix or invalidate unverified growth; inode identity and increased length alone do not satisfy the [replay-equivalence requirement](crates/beadroll-store/AGENTS.md#L60-L62).

- [P2] Capture attempt identities from the files actually replayed — /Users/<name>/Documents/beadroll/crates/beadroll-store/src/projection.rs:107-112
  If a lock-free cold read overlaps `Link::merge` replacing a differing accepted attempt, `load_listed` can read the old contents while this later `held` call captures the replacement's identity. The segment checks still pass, and the reader can overwrite the corrected projection with stale claim state or integrity reports tagged as current. Subsequent commands trust it until another journal change. Validate attempt identities across their reads, or obtain them from the opened descriptors, before saving, as required by the [cache invariant](crates/beadroll-store/AGENTS.md#L60-L62).

## roll-lqjj.2, round 2: `1527ea8ff4dc47e7d3d51954c2d1bef892b5d468..3860f0a60090c96bbe94170cd2acf8bfbd16bffb`

Reviewed only 1527ea8ff4dc47e7d3d51954c2d1bef892b5d468..3860f0a60090c96bbe94170cd2acf8bfbd16bffb (HEAD). Growth of an older segment can persist cached answers that differ from replay. All 52 prebuilt core unit tests passed; ./check.sh was blocked by the read-only Cargo build lock.

Review comment:

- [P2] Replay when a non-final cached segment grows — /Users/<name>/Documents/beadroll/crates/beadroll-store/src/projection.rs:190-195
  If an earlier segment grows while a later segment already contains cached events, this accepts an insertion into the machine's journal as an append. For example, cache create A in `000001.jsonl` and retitle B in `000002.jsonl`, then append label event C depending on B to `000001.jsonl`. `Projection::extend` places C after B and accepts it; full replay places C before B in the journal and rejects B for a chain break. The cache consequently preserves a title replay rejects and can hide `JOURNAL_FORK` from `Link::merge`. Fall back when a non-final cached segment grows, preserving the [replay-equivalence invariant](crates/beadroll-store/AGENTS.md#L60-L62).

## roll-lqjj.2, round 3: `1527ea8ff4dc47e7d3d51954c2d1bef892b5d468..a9503113fd2c80fa26a3629948bfd417f3750e54`

Reviewed only 1527ea8ff4dc47e7d3d51954c2d1bef892b5d468..a9503113fd2c80fa26a3629948bfd417f3750e54 (HEAD); no actionable defects found. All 52 prebuilt core unit tests passed. The full baseline was blocked by the read-only Cargo build lock; fresh-build and stress verification remain unperformed.

## roll-lqjj.3, round 1: `9ca05470f280f1dbacc305041b8a8f5b66d43abf..a43694b7c99ce86a50d1ffba5cfeb04d9dc53e79`

Reviewed 9ca05470f280f1dbacc305041b8a8f5b66d43abf..a43694b7c99ce86a50d1ffba5cfeb04d9dc53e79 (HEAD): no actionable defects found in the documentation-only changes. git diff --check passed; the baseline could not run because the read-only sandbox blocked Cargo’s build lock.

## roll-lqjj.3, round 2, after a correction: `dc608a8597f1269b46554686b224a0b844eea2fb..2cd9ed847c4770a1f923a8b8fea84a94d4143fb9`

The task was reopened when the census of public trackers, finished after its close, found two larger ones; this round reviews the correction alone.

Reviewed dc608a8597f1269b46554686b224a0b844eea2fb..2cd9ed847c4770a1f923a8b8fea84a94d4143fb9. No actionable defects found in this documentation-only range. Tests were not run in the read-only sandbox.

## roll-lqjj.6, round 1: `000dcec0b91f79369ad9d5c51095a0943418abb4..ecc113bd894e6109fc91e4741cdc7bf23f30cd7a`

Reviewed 000dcec0b91f79369ad9d5c51095a0943418abb4..ecc113bd894e6109fc91e4741cdc7bf23f30cd7a (HEAD). Batch catch-up introduces redundant full-text reads proportional to repeated events. The existing prebuilt binaries passed 55 core tests and 5 timing-history tests; the full baseline was not run in the read-only sandbox.

Review comment:

- [P2] Deduplicate issues before loading a batch's text blobs — /Users/<name>/Documents/beadroll/crates/beadroll-core/src/projection.rs:268-273
  When catching up multiple events for the same existing issue, `current()` and `Tx::state()` pass one UID per record. This collects every occurrence before loading anything, so `texts::read_in()` rereads the entire blob and `State::read_texts()` rehashes it for each occurrence, even after its texts are loaded. For example, 1,000 small edits to an issue with 1 MiB of accumulated text cause roughly 1 GiB of reads and hashing instead of one blob load. Deduplicate the requested UIDs, or skip already-loaded issues before performing the read.

## roll-lqjj.6, round 2: `000dcec0b91f79369ad9d5c51095a0943418abb4..8f74e62904a58eea9d03b7ff66784505ffa51da1`

Reviewed only 000dcec0b91f79369ad9d5c51095a0943418abb4..8f74e62904a58eea9d03b7ff66784505ffa51da1 (HEAD); no actionable defects found. Prebuilt binaries passed 56 core tests and five timing-history tests, and git diff --check passed. The full baseline was blocked by the read-only sandbox.

## roll-lqjj.7, round 1: `7942c353fc2d8ade230d63d06ad0e9a86f17896d..014731228568cdc4eb920076ac73419af836a0c9`

Reviewed 7942c353fc2d8ade230d63d06ad0e9a86f17896d..014731228568cdc4eb920076ac73419af836a0c9 (HEAD); no actionable defects found. Six timing unit tests passed using an existing test binary. The full baseline and contention measurements were not run because the sandbox is read-only.

## roll-lqjj.8, round 1: `47a1c76909ca9a990482f9b9557ef3a239c18651..c6e1593f141138f678a9b6e7bdb83731aa9666d5`

Reviewed 47a1c76909ca9a990482f9b9557ef3a239c18651..c6e1593f141138f678a9b6e7bdb83731aa9666d5 (HEAD); no actionable defects found. Twelve relevant tests passed using the existing xtask test binary; the full baseline and benchmarks were not run because the sandbox is read-only.
