# beadroll-sync crate guide

## Purpose

`crates/beadroll-sync` is the sync protocol of bead (`docs/RUST-PORT.md` §2.1): transactions (`Tx`,
`writing`), the link to the coordination remote (fetch, merge, tree, publish, maintenance,
rebuild), `synchronise` with its settling of this machine's pending attempts, `publish_soon` and the
unpublished counts, and the ownership protocol (`coordinated`, receipts, fences, `settle_commands`,
`settle_pending`).

## Key entrypoints

- `src/tx.rs`: `Tx` (`emit`, `state`), `locked`, `writing`, `forget_receipts`, `drop_pending`. A
  `Tx` starts from the store's projection and applies each event it emits to it; one the
  projection cannot take (a takeover) has the journal replayed instead.
- `src/link.rs`: `Link` (`unpublished`, `maintain`, `rebuild`) and the gits it keeps in
  `sync.git` (`docs/RUST-PORT.md` Appendix B); `src/link/fetch.rs`: `Link::fetch`, which tries
  again a fetch that lost to another process's fetch of the store; `src/link/asked.rs`: `Link::publishes`, the
  `ls-remote` that `init` asks before it starts a tracker, as a link asks but from a bare
  repository made aside, writing nothing in the store; `src/link/merge.rs`: `Link::merge`;
  `src/link/publish.rs`: its tree and `Link::publish`; `src/link/read.rs`: the remote's tree read
  through the link's `cat-file`; `src/link/write.rs`: blobs, trees and the ref after a push through
  its `hash-object`, `mktree` and `update-ref`, with separate ref updates after a fetch.
  Each falls back to the corresponding separate git command. `Link::local_blobs` hashes only
  journal files the store's memo (`beadroll_store::Memo`) cannot vouch for; `Link::hashed` lists them
  (roll-lqjj.1); `Link::unpublished` and `Link::published_counts` take a segment's lines from the
  memo while the segment is unchanged.
- `src/synchronise.rs`: `synchronise` and `Synced`, `adopt_published`, `publish_soon`,
  `unpublished_count`, `pause`, `ROUNDS`.
- `src/ownership.rs`: `coordinated` (the twelve rounds of a claim, takeover, release or claiming
  update), `settle_commands` (receipts), `settle_pending` (fences), `forget`, `Mode`, `Settled`.
- `src/error.rs`: `Error`, each variant with its code.
- `tests/sync/`: in-crate protocol tests; `support.rs` opens a machine's store in the test process
  and runs command bodies through `coordinated`. Faults come from failpoints passed in `Settings`; the
  machines around it run `BEAD_UNDER_TEST`, else the Rust `bead` Cargo built in the profile
  directory above the test binary (`beadroll_test_support::built_bead`, below).

## Edit here for X

- How a write, a sync or a claim reaches the remote, and what each outcome means: here.
- Receipts and pending attempts: here, preserving the frozen format byte for byte (§2.3j).

## Invariants

- Exactly 12 rounds per command, `pause(U(0.1, 0.4) × attempt)` after a failed one, outside the
  store lock (§1.3).
- The protocol depends on fetch, read-tree and push, never on `sync.git`'s internals (§2.3i); a
  `Remote` trait waits for its second implementation.
- A link preserves git operation order and failure classification (`Link::git` may find damage,
  `Link::check` never does). A kept git that has no answer (`beadroll_git::NoAnswer`) falls back
  to the corresponding separate git command. It does not hash a journal file
  that keeps the identity the memo remembers it at: a merge compares the remembered blob with the
  published one as it is, and a publish names it in a tree only when this link made it or the tree
  of the tip it merged holds it at that path, so that a blob a gc dropped is never named in a
  tree. Its `cat-file` starts before a fetch, its `hash-object` and `mktree` after it, its
  `update-ref` before a publish's `commit-tree`, and all end with the link or when `sync.git` is
  set aside (`rebuild`, a stale format).
- The fetch's own ref (`refs/beads/fetch/<pid>-<hex>`) is deleted by a separate
  `update-ref -d` after `refs/beads/remote`'s, never in a transaction ready the moment the fetch
  ends: concurrent fetches of the store list every ref then, and git fails one that finds a listed
  ref gone (G16, `docs/RUST-PORT.md` Appendix B).
- A transaction reads where the caller works (`capture_where`) when it starts, under the store
  lock, and an ownership command makes a new one each round: the `where` an
  event records is HEAD as it is then.
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
- Receipt signatures and retirement preserve compatibility across versions (§2.3j,
  `REVIEWING.md` §2); sync keeps an accepted attempt's receipt.
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
