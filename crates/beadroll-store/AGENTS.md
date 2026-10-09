# beadroll-store crate guide

## Purpose

`crates/beadroll-store` is this machine's side of bead (`docs/RUST-PORT.md` §2.1): `BEAD_HOME`, the
store's files and modes, the lock, atomic writes, journal segments and torn tails, attempt, pending
and receipt files, identity and aliases, workspace discovery and remote resolution, actor detection,
`view`, and the failpoint registry. Slice 2a (`roll-k2wy.3`) fills it; until then it is empty.

## Key entrypoints

- `src/lib.rs`: the crate root, and so far all of it.

## Edit here for X

- A file in the store, its name, mode or how it is written: here.
- Workspace discovery, identity, actor detection: here.
- A failpoint's registry and parsing (§2.3g, §4.5): here; the points themselves sit where they fire.

## Invariants

- The bytes on disk are format 1's, exactly as Python writes them (§1.1); format 1 is frozen.
- Directories 0700 and files 0600 throughout the store, whatever the umask; files written into a
  checkout follow the umask.
- Atomic writes use Python's temporary names and `fsync(2)` through `rustix`, not `sync_all`
  (§2.3e); appends use `O_APPEND`.
- Settings arrive as values (`Settings`); this crate never reads the process environment (§2.2).
- It depends on `beadroll-core` and `beadroll-git` only.

## Common commands

- `cargo test -p beadroll-store`
- `cargo clippy -p beadroll-store --all-targets -- -D warnings -D clippy::mod_module_files`
