# beadroll-sync crate guide

## Purpose

`crates/beadroll-sync` is the sync protocol of bead (`docs/RUST-PORT.md` §2.1): transactions (`Tx`,
`writing`), the link to the coordination remote (fetch, merge, tree, publish, maintenance,
rebuild), `synchronise`, the ownership protocol (`coordinated`, receipts, fences, settling),
`publish_soon` and the unpublished counts. Slices 2a and 2c (`roll-k2wy.3`, `.5`) fill it; until
then it is empty.

## Key entrypoints

- `src/lib.rs`: the crate root, and so far all of it.

## Edit here for X

- How a write, a sync or a claim reaches the remote, and what each outcome means: here.
- Receipts and pending attempts: here, compatible with Python's byte for byte (§2.3j).

## Invariants

- Exactly 12 rounds per command, `pause(U(0.1, 0.4) × attempt)` after a failed one, outside the
  store lock (§1.3).
- The protocol depends on fetch, read-tree and push, never on `sync.git`'s internals (§2.3i); a
  `Remote` trait waits for its second implementation.
- Receipt signatures and retirement stay Python's while either implementation may use a store,
  rollback included (§2.3j, `REVIEWING.md` §2).
- It depends on `beadroll-core`, `beadroll-git` and `beadroll-store` only.

## Common commands

- `cargo test -p beadroll-sync`
- `cargo clippy -p beadroll-sync --all-targets -- -D warnings -D clippy::mod_module_files`
