# beadroll-core crate guide

## Purpose

`crates/beadroll-core` is the pure core of bead (`docs/RUST-PORT.md` §2.1): Python-compatible JSON
and text semantics, event ids and times, event validation, journal settling, causal order, replay
into state, readiness and the other queries, issue views, Beads import planning and export, key
minting, and reading a published tree. pancake-studio links this crate alone. Phase 1
(`roll-k2wy.2`) fills it; until then it is empty.

## Key entrypoints

- `src/lib.rs`: the crate root, and so far all of it.

## Edit here for X

- Reading or writing JSON the way Python does (§2.3a, §5): here.
- Replay, state, readiness, cycles, views, import planning and export: here.
- Anything that reads a file, starts a process, or asks the clock or the environment: not here,
  but `beadroll-store`, `beadroll-git`, or the CLI, which passes the value down.

## Invariants

- No I/O: no filesystem, processes, clock or environment (§2.2).
- It depends on no other workspace crate; any of them may depend on it.
- No panics: `unwrap`, `expect`, `panic!`, `todo!` and `unimplemented!` are denied, and replay
  is total: every input is applied or reported.
- No `HashMap` or `HashSet` iteration order reaches an output; issues keep replay order.
- Its answers are Python's, proven differentially against CPython (`xtask diff`, §4.2); a
  departure is a decided row in §5, never a drift.

## Common commands

- `cargo test -p beadroll-core`
- `cargo clippy -p beadroll-core --all-targets -- -D warnings -D clippy::mod_module_files`
