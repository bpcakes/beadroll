# beadroll-store crate guide

## Purpose

`crates/beadroll-store` is this machine's side of bead (`docs/RUST-PORT.md` §2.1): the settings the
CLI hands down, `BEAD_HOME`, the store's files and modes, the lock's file, atomic writes, journal
segments and torn tails, attempt and pending files, identity and aliases, workspace discovery and
remote resolution, actor detection, and `view`. Slice 2a (`roll-k2wy.3`) built it; Slice 2c adds the
failpoint registry.

## Key entrypoints

- `src/settings.rs`: `Settings` (the variables, the environment git inherits, the working directory,
  the clock; `home()` with the `BEAD_HOME` rules), `Clock`, `pathlib`.
- `src/fs.rs`: `private_dirs`, `private_dir`, `write_atomic`, `fsync`, `dir_entries`, `glob_ending`,
  `read_json_text` and Python's error texts (`os_error_str`, `utf8_error`).
- `src/identity.rs`: `replica_id`, `machine_aliases`, `uuid7`, `hex_token`.
- `src/actor.rs`: `detect_actor`, `nearest_harness` (its `ps` passed in by the CLI).
- `src/workspace.rs`: `Workspace` (`meta`, `save_meta`, `note_sync`, `ref_name`, `resolve_remote`,
  `remote`), `stamp`; `src/discover.rs`: `find_workspace`, `checked_config`, `moved_workspaces`.
- `src/journal.rs`: segment, attempt and pending files, `append_events`, `repair_tail`,
  `load_journal`, `load_pending`, `view`; `src/place.rs`: `capture_where`, `merged_into_default`.
- `tests/store/`: the Python suite's tests of the store, under the inventory's naming rule.

## Edit here for X

- A file in the store, its name, mode or how it is written: here.
- Workspace discovery, identity, actor detection: here.
- A failpoint's registry and parsing (§2.3g, §4.5): here, from Slice 2c; the points themselves sit
  where they fire.

## Invariants

- The bytes on disk are format 1's, exactly as Python writes them (§1.1); format 1 is frozen.
- Directories 0700 and files 0600 throughout the store, whatever the umask; files written into a
  checkout follow the umask.
- Atomic writes use Python's temporary names and `fsync(2)` through `rustix`, not `sync_all`
  (§2.3e); appends use `O_APPEND`.
- Settings arrive as values (`Settings`); this crate never reads the process environment or the
  working directory (§2.2). A numeric `BEAD_*` setting that is not a number is a `CONFIG` error
  when the settings are built (§5 row 26).
- It depends on `beadroll-core` and `beadroll-git` only, of the workspace.

## Common commands

- `cargo test -p beadroll-store`
- `cargo clippy -p beadroll-store --all-targets -- -D warnings -D clippy::mod_module_files`
