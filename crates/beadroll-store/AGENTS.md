# beadroll-store crate guide

## Purpose

`crates/beadroll-store` is this machine's side of bead (`docs/RUST-PORT.md` §2.1): the settings the
CLI hands down, `BEAD_HOME`, the store's files and modes, the lock's file, atomic writes, journal
segments and torn tails, attempt and pending files, identity and aliases, workspace discovery and
remote resolution, actor detection, and `view`. Slice 2a (`roll-k2wy.3`) built it; Slice 2c
(`roll-k2wy.5`) added the failpoint registry.

## Key entrypoints

- `src/settings.rs`: `Settings` (the variables, the environment git inherits, the working directory,
  the clock; `home()` with the `BEAD_HOME` rules; `checkout` and `fact`: what checkout-side git says
  of the repository, asked once, `docs/RUST-PORT.md` Appendix B), `Clock`, `normalize_path`,
  `join_path`.
- `src/fs.rs`: `private_dirs`, `private_dir`, `write_atomic`, `fsync`, `dir_entries`, `glob_ending`,
  `read_json_text`, `read_json_keeping_surrogates` (a file Python rewrites whole, or whose names
  print as Python prints them) and Python's error texts (`os_error_str`, `utf8_error`).
- `src/identity.rs`: `replica_id`, `machine_aliases`, `uuid7`, `hex_token`.
- `src/actor.rs`: `detect_actor`, `nearest_harness` (its `ps` passed in by the CLI); an `Actor` is
  its JSON value, whether it can write (not when Python would hold its text as lone surrogates),
  and its `label` (Python's `actor_label` over the bytes it was given): claims are compared by
  `Label::is`, never by the printed text, so an actor that is not UTF-8 holds nothing.
- `src/failpoint.rs`: `Failpoints`, the registry `Settings` carries (`BEAD_FAILPOINTS` and the two
  crash hooks): `hit`, `hit_path` for `atomic-write:`/`atomic-written:`, `has`, and `hits`, which
  in-crate tests read to check that a fault fired; `POINTS`, every point bead has.
- `src/workspace.rs`: `Workspace` (`meta`, `save_meta`, `note_sync`, `ref_name`), `stamp`;
  `src/workspace/remote.rs`: what a remote given to bead means (`resolve_endpoint`, `endpoint`,
  an `Endpoint` being the URL, whether it is a remote's own push URL and the name it was chosen
  by; `Endpoint::chosen`, the remote as it was chosen, which a store remembers and compares and
  advice gives; `resolve_remote` and `remote`, their URLs; `remembered_remote`, `is_origin_url`)
  and a remote's URL as git is configured with it (`configured_url`, which discovery reads origin
  with); `src/discover.rs`: `find_workspace`, `checked_config`, `moved_workspaces`,
  `tracked_config_warning` (a `.beadroll.json` the other checkouts do not see), `workspaces_named`.
- `src/journal.rs`: segment, attempt and pending files, `append_events`, `repair_tail` (a segment
  that ends with a newline is whole, and only its last byte is read), `load_journal`,
  `load_pending`, `view`; `src/place.rs`: `capture_where`, `merged_into_default`.
- `src/projection.rs`: `projection` and the state `view` reads (roll-lqjj.2): the store's
  projection, `<store>/projection`, read when the journal's files are the ones it was made from,
  caught up when records were only appended to them, else the journal replayed, and written back
  when the files held still; `src/projection/file.rs`: its header, sums and what it was made from.
- `src/memo.rs`: `Memo`, what the store remembers of each journal file between commands (its
  identity on disk, `FileKey`, its lines and its blob in `sync.git`), kept as
  `sync.git/journal-memo` (`memo_path`) by the holder of the store lock (roll-lqjj.1).
- `tests/store/`: the Python suite's tests of the store, ported under the naming rule of
  `docs/RUST-PORT.md` §4.2.

## Edit here for X

- A file in the store, its name, mode or how it is written: here.
- Workspace discovery, identity, actor detection: here.
- A failpoint's registry and parsing (§2.3g, §4.5): `src/failpoint.rs`; the points themselves sit
  where they fire. A new point joins `POINTS`, and an action a point can take joins `allowed`.

## Invariants

- The bytes on disk are format 1's, exactly as Python writes them (§1.1); format 1 is frozen.
  The memo is no part of it: a cache in `sync.git` that goes with it, and a memo that is
  missing, cannot be read or names a file whose identity changed counts for nothing. Nor is the
  projection: a cache that one build of bead (`Settings::build`) reads and writes, whole or not
  at all, never into a store that is not there; a damaged, cut, foreign or stale one is replayed
  past, and the answers are replay's either way.
- Directories 0700 and files 0600 throughout the store, whatever the umask; files written into a
  checkout follow the umask.
- Atomic writes use Python's temporary names and `fsync(2)` through `rustix`, not `sync_all`
  (§2.3e); appends use `O_APPEND`.
- Settings arrive as values (`Settings`); this crate never reads the process environment or the
  working directory (§2.2). A numeric `BEAD_*` setting that is not a number is a `CONFIG` error
  when the settings are built (§5 row 26).
- The repository's top, common directory and format are asked through `Settings::fact` (one
  `rev-parse`, each answer what Python's single call prints), never one call a question; the
  default coordination ref is valid without `check-ref-format`. `git` is set before the first
  question: the answers are its environment's. Nothing that can change while bead runs is kept:
  `find_workspace` asks origin's URLs each time it runs (`Checkout::origin_urls`, Python's own
  call), and `capture_where` asks HEAD and its branch each time it runs (`head_and_branch`):
  Python's `Tx` asks when each transaction starts, under the store lock, and the event records
  what HEAD is then.
- Failpoints reach code through `Settings` only, never the process environment; anything they cannot
  do as written (an unknown point, an action the point cannot take, a second spec for one point)
  is `CONFIG` when the settings are built.
- It depends on `beadroll-core` and `beadroll-git` only, of the workspace.

## Common commands

- `cargo test -p beadroll-store`
- `cargo clippy -p beadroll-store --all-targets -- -D warnings -D clippy::mod_module_files`
