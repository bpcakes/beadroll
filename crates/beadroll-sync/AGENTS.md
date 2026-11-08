# beadroll-sync crate guide

## Purpose

`crates/beadroll-sync` is the sync protocol of bead: transactions (`Tx`, `writing`), the link to
the coordination remote (fetch, merge, tree, publish, maintenance, rebuild), `synchronise` with
its settling of this machine's pending attempts, `publish_soon` and the unpublished counts, and
the ownership protocol (`coordinated`, receipts, fences, `settle_commands`, `settle_pending`).

## Key entrypoints

- `src/tx/attempt.rs`: `Tx::attempt` lends an `Attempt` writer that routes edits with the claim
  without changing the ordinary transaction's mode. `Tx::conditional` remains a compatibility
  switch. `locked` uses the store's `WorkspaceLock` guard for both the OS lock and cache phase.
- `src/link/merge.rs`: `Merging` owns incomplete per-round facts; only a successful merge yields
  `Merged` (checked state, incoming count and report facts). Internal callers consume it; public
  `Link::merge` retains its count and report-field interface. Rename baseline and recovery remain
  on the command's link/loop, while the report consumes the successful round's snapshot. The public
  wrapper keeps only opaque `Spent` until the next merge or link destruction, deferring disposal
  when its caller keeps the link outside the lock.

- `src/tx/prepared.rs`: consuming `Tx::prepare` separates `PreparedWrite` (events to persist,
  with no emitter or applied prefix) from `Spent` (the retained materialized view). Both local
  and coordinated paths retain these outside the lock on errors as well as success.

- `src/tx.rs`: `Tx` (`emit`, `state`, `lend`, `on`), `Emitter` and `Spent`. A `Tx` starts from the
  store's projection (`on`: one the caller read under the lock it holds, with the journal files it
  was made from (`Sourced`); read again, as `new` does, when a file was written since,
  roll-lqjj.37) and applies each event it emits to it; one the projection cannot take (a takeover)
  has the journal replayed instead. `lend` lends the state beside an `Emitter`, for a command that
  reads it while it emits, without a copy (roll-lqjj.29). `src/tx/writes.rs`: `locked`, `writing`,
  `writing_state`, which hands back the state a write leaves, `writing_spent`, which hands back
  what its transaction held (`Spent`), for a caller whose process ends with its command to keep
  (roll-lqjj.42), and `drop_pending`. `src/tx/materialized.rs` keeps the projection, replay-needed
  and replayed states mutually exclusive, each cached view with its applied event prefix.
- `src/link.rs`: `Link` (`unpublished`, `maintain`, `rebuild`) and the gits it keeps in
  `sync.git` (`crates/beadroll-git/AGENTS.md`); `src/link/fetch.rs`: `Link::fetch`, which tries
  again a fetch that lost to another process's fetch of the store; `src/link/asked.rs`: `Link::publishes`, the
  `ls-remote` that `init` asks before it starts a tracker, as a link asks but from a bare
  repository made aside, writing nothing in the store; `src/link/merge.rs`: `Link::merge`;
  `src/link/publish.rs`: its tree and `Link::publish`; `src/link/read.rs`: the remote's tree read
  through the link's `cat-file`, and `Link::lost`, what of a fetched tip a failed read of it finds
  missing or damaged in `sync.git` (roll-dih8.30); `src/link/write.rs`: blobs, trees and the ref
  after a push through its `hash-object`, `mktree` and `update-ref`, with one ref transaction
  after a fetch.
  Each falls back to the corresponding separate git command. `Link::local_blobs` hashes only
  journal files the store's memo (`beadroll_store::Memo`) cannot vouch for, an attempt file by the
  identity its locked phase took of it (`beadroll_store::claim_keys`, roll-lqjj.30); `Link::hashed`
  lists them (roll-lqjj.1); `Link::unpublished` and `Link::published_counts` take a segment's
  lines from the memo while the segment is unchanged.
- `src/link/helpers.rs`: the reader, restartable hash/ref writers and publication-owned tree
  batches, with fetch and repository replacement boundaries. `src/link/known.rs`: object-presence
  evidence and its invalidation by merge, rehash, maintenance and repository replacement.
- `src/synchronise.rs`: `synchronise` and `Synced`, `adopt_published`, `publish_soon` (and its
  two halves, `destination`, where a write would go, and `publish_soon_at`; roll-lqjj.50),
  `unpublished_count` (the memo's segment entries alone, `meta.json` as the command read it, the
  attempt files named by their bytes with `relative_name`; roll-lqjj.30 and roll-lqjj.47),
  `pause`, `ROUNDS`. `src/synchronise/round.rs` returns each round's incoming progress beside
  its publication or contention outcome, even on failure, and names the recovery decisions.
- `src/ownership.rs`: `coordinated` (up to 24 rounds of a claim, takeover, release or claiming
  update), `settle_pending` (fences), `Mode`, `Settled`;
  the failpoint `ownership.before-transaction` sits between a round's merge (and what settles after
  it) and its transaction, where nothing writes the journal. `src/ownership/attempt.rs` owns the
  prepared attempt, its publication outcome and the answer recovery retains across rounds.
- `src/receipts.rs`: `settle_commands`, `forget`, `forget_receipts`; receipt classification and
  immediate, recognised-retry or deferred retirement. Public compatibility entrypoints retain
  their signatures; `coordinated` uses the named request result and deferred retirement batch.
- `src/error.rs`: `Error`, each variant with its code.
- `tests/sync/`: in-crate protocol tests (`count.rs` and `count_foreign.rs`: the unpublished count
  against a count of the journal the slow way, on one machine and on two, on every path and
  whatever the memo holds); `support.rs` opens a machine's store in the test process and runs
  command bodies through `coordinated`. Faults come from failpoints passed in `Settings`; the
  machines around it run `BEAD_UNDER_TEST`, else the Rust `bead` Cargo built in the profile
  directory above the test binary (`beadroll_test_support::built_bead`, below).

## Edit here for X

- How a write, a sync or a claim reaches the remote, and what each outcome means: here.
- Receipts and pending attempts: here, preserving the frozen format byte for byte
  (`docs/REVIEWING.md` §1).

## Invariants

- A transaction, which holds the store's whole projection, is let go of after the store lock is,
  never inside it: `writing` and each round of `coordinated` keep theirs outside `locked` and drop
  it when it returns (roll-lqjj.32). `writing_spent` hands what it held to its caller instead, who
  keeps it or frees it; this crate never leaks it, on a write that fails either (roll-lqjj.42;
  `xtask/tests/heap` sees `run_with` return holding none of it after `writing`, a failed write and
  a round of `coordinated`). `src/tx/tests.rs` observes the real file lock at materialized-view
  destruction on failed builds and persistence, with projection and replay, for both write paths.
  Its test-only probe follows the view through preparation; a locked-drop control checks that the
  probe detects the forbidden order. It is attached after the body's final state read, so it does
  not observe replacement during event application.
- Up to 24 rounds per command, with a uniformly sampled pause from zero up to
  `min(0.1 × 2^(attempt − 1), 2)` seconds after a failed round, outside the store lock
  (roll-lqjj.7; `docs/REVIEWING.md` §2).
- Sync retries contention after releasing the lock; a damaged round spends its single rebuild
  allowance under the lock and proceeds without backoff. Adoption is considered only on round
  one. Incoming counts survive a failed publication and rebuild; outgoing counts and maintenance
  come from the successful round. The rebuild allowance is independent of the path in the report.
- The protocol depends on fetch, read-tree and push, never on `sync.git`'s internals
  (`docs/REVIEWING.md` §2); a
  `Remote` trait waits for its second implementation.
- A link preserves git operation order and failure classification (`Link::git` may find damage,
  `Link::run` leaves its output to the caller). A failed read of the fetched tip (manifest,
  listing, blob) asks `Link::lost` what of the tip is missing or damaged in `sync.git`, and that is
  damage: a prune can remove what a fetch brought before it is read, and a tree or blob gone under
  a commit still there is never fetched again; a blob there but unreadable, or a tree git cannot
  walk, is damage too. Every tree in the merge's listing is checked against its name; a mismatch
  is local damage even when git can read it. The standalone ls-tree fallback includes trees,
  including empty ones, and verifies their hashes too (roll-dih8.31). A git a signal ended is never
  damage (`Output::killed`).
  Failed reads also check the root's hash: a wrong-name root may hide the manifest or prevent
  ls-tree while rev-list still walks it. `rev-parse TIP:` names it without the content check
  that makes `TIP^{tree}` fail, then `cat-file tree` supplies the bytes, even without a kept reader.
  A kept git that has no answer (`beadroll_git::NoAnswer`) falls back to the corresponding
  separate git command. It does not hash a journal file
  that keeps the identity the memo remembers it at: a merge compares the remembered blob with the
  published one as it is, and a publish names it in a tree only when this link made it or the tree
  of the tip it merged holds it at that path, so that a blob a gc dropped is never named in a
  tree. Its `cat-file` starts before a fetch, its `hash-object` and `mktree` after it, its
  `update-ref` after a successful fetch (before `commit-tree` if no head was fetched), and all
  end with the link or when `sync.git` is set aside (`rebuild`, a stale format).
  Hash and ref writers that cannot answer restart on their next request; a failed tree batch
  stays on standalone mktree for the rest of that publication unless the rehash fallback explicitly
  retries it. Ending helpers happens before a repository rename, including one that fails.
- One `update-ref` transaction keeps the fetched head in `refs/beads/remote` and deletes the
  fetch's private ref, reusing that git after a push (roll-dih8.14). A failed transaction falls
  back to the established separate calls. Deleting the private ref can race another fetch's
  checks of all refs; that fetch retries under the bounded policy in `link/fetch.rs`.
- A publish can name a claims subtree without rebuilding it only when the last merged tip holds
  it at that path and its object name equals the desired entries' canonical tree name. Merge,
  rebuild and the rehash fallback clear candidates; `Link::reused_trees` records reuse. It changes
  no published bytes (roll-lqjj.13).
- A transaction reads where the caller works (`capture_where`) when it starts, under the store
  lock, and an ownership command makes a new one each round: the `where` an
  event records is HEAD as it is then.
- A round reads the store's state once, by its merge, which keeps it (`Link::merged`, a
  `Sourced`: the projection and the journal files it was made from): an ownership round's
  transaction starts from it, and a sync's report is made of the last one. A transaction starts from
  it only while those files are the store's (`Tx::on` asks `Sourced::into_current` and falls back to
  `Tx::new`, which reads again): a step added between the merge and the transaction may write the
  journal, and the round then reads the store again, which the `reads::` and `protocol::merged_window`
  tests show with `ownership.before-transaction`. A
  sync that reports counts the renames before it from a projection its first merge reads before
  merging, which that merge goes on with when it writes no journal file; every journal write of a
  merge goes through `write_journal` or `wrote`, which mark it. A write's sync (`publish_soon`)
  makes no report (roll-lqjj.29; the `reads::` tests count them at `projection.read`).
- A link asks for the checkout's transport settings when it is made (`Link::new`,
  `beadroll_git::transport_settings`). A link is made at the start of `synchronise`, another if
  it adopts a published tracker, one before the rounds of `coordinated`, and one for a write
  through `publish_soon` once its
  transaction has let go of the store lock. A setting changed while bead waited is the one the
  link uses. A link's git starts at the top of the checkout, where bead reads a relative remote
  path from, so that one git makes of a rewritten URL is read from there too. A link is made for an
  `Endpoint` (`Workspace::endpoint`), and every git that reaches the remote (its fetch, its push,
  `init`'s `ls-remote`) is given it as a remote defined for the link under a name of its own
  (`defining_remote`, through `GIT_CONFIG_COUNT`), its own push URL as its `pushurl`: git never
  takes the URL for another remote's name, and pushes as `git push NAME` would.
- Receipt signatures and retirement preserve compatibility across versions
  (`docs/REVIEWING.md` §1 and §2); sync keeps an accepted attempt's receipt.
  `crates/beadroll/tests/cli/parity/receipts.rs` pins the format bytes.
- It depends on `beadroll-core`, `beadroll-git` and `beadroll-store` only, of the workspace.
- The machines of its tests run the `bead` the workspace builds, never a path worked out from the
  source tree: `BEAD_UNDER_TEST` when it is set, else `built_bead()`, the `bead` in the profile
  directory above the test binary, which is wherever Cargo put this build (target directory,
  build target, profile). A run that builds the `beadroll` package makes it fresh (`cargo nextest
  run --workspace`); one of this package alone does not, and uses whatever `bead` an earlier build
  left there, so build it first. Without one, the first sandbox fails the test and says how.

## Common commands

- `cargo build -p beadroll && cargo nextest run -p beadroll-sync`: the in-crate tests, the
  machines around them running that `bead` (`link::`'s count of the test process's children and
  descriptors needs a process of its own, which `cargo test` does not give it)
- `cargo nextest run --workspace`: the same with every other crate's tests, `bead` built first
- `cargo clippy -p beadroll-sync --all-targets -- -D warnings -D clippy::mod_module_files`
