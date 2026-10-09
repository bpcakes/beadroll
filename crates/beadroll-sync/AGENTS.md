# beadroll-sync crate guide

## Purpose

`crates/beadroll-sync` is the sync protocol of bead (`docs/RUST-PORT.md` §2.1): transactions (`Tx`,
`writing`), the link to the coordination remote (fetch, merge, tree, publish, maintenance,
rebuild), `synchronise` with its settling of this machine's pending attempts, `publish_soon` and the
unpublished counts. Slice 2a (`roll-k2wy.3`) built these; Slice 2c (`roll-k2wy.5`) adds the
ownership protocol (`coordinated`, receipts, fences, `settle_commands`, `settle_pending`).

## Key entrypoints

- `src/tx.rs`: `Tx` (`emit`, `state`), `locked`, `writing`, `forget_receipts`, `drop_pending`.
- `src/link.rs`: `Link` (`fetch`, `unpublished`, `maintain`, `rebuild`); `src/link/merge.rs`:
  `Link::merge`; `src/link/publish.rs`: its tree and `Link::publish`.
- `src/synchronise.rs`: `synchronise` and `Synced`, `adopt_published`, `publish_soon`,
  `published_counts`, `unpublished_count`, `pause`, `ROUNDS`.
- `src/error.rs`: `Error`, each variant with its code.

## Edit here for X

- How a write, a sync or a claim reaches the remote, and what each outcome means: here.
- Receipts and pending attempts: here, compatible with Python's byte for byte (§2.3j).

## Invariants

- Exactly 12 rounds per command, `pause(U(0.1, 0.4) × attempt)` after a failed one, outside the
  store lock (§1.3).
- The protocol depends on fetch, read-tree and push, never on `sync.git`'s internals (§2.3i); a
  `Remote` trait waits for its second implementation.
- Receipt signatures and retirement stay Python's while either implementation may use a store,
  rollback included (§2.3j, `REVIEWING.md` §2); sync keeps an accepted attempt's receipt.
- It depends on `beadroll-core`, `beadroll-git` and `beadroll-store` only, of the workspace.

## Common commands

- `cargo test -p beadroll-sync`
- `cargo clippy -p beadroll-sync --all-targets -- -D warnings -D clippy::mod_module_files`
- The mixed-fleet tests that drive this crate against Python: `cargo nextest run -p beadroll -E
  'test(/^mixed::/)'`
