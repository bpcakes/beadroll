# beadroll-store crate guide

## Purpose

`crates/beadroll-store` is this machine's side of bead: the settings the CLI hands down,
`BEAD_HOME`, the store's files and modes, the lock's file, atomic writes, journal segments and torn
tails, attempt and pending files, identity and aliases, workspace discovery and
remote resolution, actor detection, `view`, and the failpoint registry.

## Key entrypoints

- `src/settings.rs`: `Settings` (the variables, the environment git inherits, the working directory,
  the clock; `home()` with the `BEAD_HOME` rules; `checkout` and `fact`: what checkout-side git says
  of the repository, asked once, `crates/beadroll-git/AGENTS.md`), `Clock`, `normalize_path`,
  `join_path`.
- `src/fs.rs`: `private_dirs`, `private_dir`, `write_atomic`, `write_cache_parts`, `fsync`, `dir_entries`, `glob_ending`,
  `read_json_text`, `read_json_keeping_surrogates` (preserving lone surrogates across whole-file
  rewrites) and file-read diagnostics (`os_error_str`, `utf8_error`).
- `src/identity.rs`: `replica_id`, `machine_aliases`, `uuid7`, `hex_token`.
- `src/actor.rs`: `detect_actor`, `nearest_harness` (its `ps` passed in by the CLI); an `Actor` is
  its JSON value, whether it can write (only with valid UTF-8 text), and its `label` derived from
  the original bytes: claims are compared by `Label::is`, never by the printed text, so an actor
  that is not UTF-8 holds nothing.
- `src/failpoint.rs`: `Failpoints`, the registry `Settings` carries (`BEAD_FAILPOINTS` and the two
  crash hooks): `hit`, `hit_path` for `atomic-write:`/`atomic-written:`, `has`, and `hits`, which
  in-crate tests read to check that a fault fired; `POINTS`, every point bead has.
- `src/workspace.rs`: `Workspace` (`meta`, read once a command while the file keeps its identity,
  `save_meta`, `note_sync`, `ref_name`), `stamp`;
  `src/workspace/remote.rs`: what a remote given to bead means (`resolve_endpoint`, `endpoint`,
  an `Endpoint` being the URL, whether it is a remote's own push URL and the name it was chosen
  by; `Endpoint::chosen`, the remote as it was chosen, which a store remembers and compares and
  advice gives; `resolve_remote` and `remote`, their URLs; `remembered_remote`, `is_origin_url`)
  and a remote's URL as git is configured with it (`configured_url`, which discovery reads origin
  with); `src/discover.rs`: `find_workspace`, `checked_config`, `moved_workspaces`,
  `tracked_config_warning` (a `.beadroll.json` the other checkouts do not see), `workspaces_named`.
- `src/journal.rs`: segment, attempt and pending files, `append_events`, `repair_tail` (a segment
  that ends with a newline is whole, and only its last byte is read), `load_journal`,
  `load_pending`, `view`, `pending_listed` (what `view` makes of a state a caller has already,
  roll-lqjj.29); `src/place.rs`: `capture_where`, `merged_into_default` (every commit a command
  shows, asked of git at once: roll-lqjj.26);
  `src/elsewhere.rs`: `elsewhere`, the branches and worktrees whose copy of a file (Beads'
  `.beads/issues.jsonl`) has changes the checkout lacks, for an import's dry run.
- `src/projection.rs`: `projection` and the state `view` reads (roll-lqjj.2): the store's
  projection, `<store>/projection`, read when the journal's files are the ones it was made from,
  caught up when records were only appended to them, else the journal replayed, and written back
  when the files held still; `src/projection/file.rs`: its header, sums and what it was made from;
  `src/projection/sources.rs`: `Sourced`, a projection with the journal files it was made from
  (`projection_sourced`: each segment's identity as `current` read it, the attempt files by the
  listing and identities the locked phase keeps), and `Sourced::into_current`, which says whether
  they are still the store's files (roll-lqjj.37).
- `src/listing.rs`: `Claims`, the in-memory accepted-attempt names shared by `claim_files`,
  validated against each directory's identity on every call and cleared at lock boundaries.
  A directory change re-lists it; the file identities used by the memo and projection are still
  checked individually (roll-lqjj.12): `claim_keys` stats each file once a locked phase, whoever
  asks, and each time outside the lock (roll-lqjj.30). `claim_files_afresh` is the listing a
  reader takes once it has read the segments, every directory read again outside the lock (X13),
  with the `Stamp` of the listing: the identity each claims directory had when it was listed,
  which tells a later reader that a file was added, removed or renamed over, whoever wrote it
  (roll-lqjj.37).
- `src/memo.rs`: `Memo`, what the store remembers of each journal file between commands (its
  identity on disk, `FileKey`, its lines and its blob in `sync.git`), kept as
  `sync.git/journal-memo` (`memo_path`) by the holder of the store lock (roll-lqjj.1);
  `Memo::load_only`, the entries of a few files, the memo still checked whole, never written back.
- `tests/store/`: store files, settings, discovery, compatibility and damage handling.

## Edit here for X

- A file in the store, its name, mode or how it is written: here.
- Workspace discovery, identity, actor detection: here.
- A failpoint's registry, parsing and the list of points and actions: `src/failpoint.rs`; the
  points themselves sit where they fire. A new point joins `POINTS` and the list in the module's
  docs, and an action a point can take joins `allowed`.

## Invariants

- The bytes on disk follow the frozen format 1 (`docs/REVIEWING.md` §1), readable by every
  compatible version.
  The memo is no part of it: a cache in `sync.git` that goes with it, and a memo that is
  missing, cannot be read or names a file whose identity changed counts for nothing. Nor is the
  projection: a cache that one build of bead (`Settings::build`) reads and writes, whole or not
  at all, never into a store that is not there; a damaged, cut, foreign or stale one is replayed
  past, and the answers are replay's either way.
- Directories 0700 and files 0600 throughout the store, whatever the umask; files written into a
  checkout follow the umask.
- Atomic writes use the established temporary names and `fsync(2)` through `rustix`, not `sync_all`
  (`docs/REVIEWING.md` §2); appends use `O_APPEND`. The one exception is `write_cache_parts`, for
  the projection and its text file (and the text file's appends): checksummed caches, renamed in
  whole but not fsynced, whose every reader checks them against their sums (`docs/REVIEWING.md`
  §2, the durability bullet). A new file that is read without a sum to check, or that is a
  source of anything, is written with `write_atomic`.
- A caller that goes on from a projection it read earlier in a locked phase takes it with
  `projection_sourced` and asks `Sourced::into_current` before it does, and reads the store again
  when that says a file was written since (`Tx::on` does, roll-lqjj.37). Identities suffice
  because every write bead makes to a journal file changes a length (an append) or an inode (a file
  renamed into place) or the set of files. The check stats each segment again, and takes the
  attempt files' listing and identities from the phase, with the identity each claims directory had
  when it was listed (`Claims::stamp`): a file added, removed or renamed over changes its
  directory's, so a write is seen whether the writer told the phase (`write_attempt_file`; the
  merge's `write_journal` with a separate `forget_claims`) or not (a bare `write_atomic`), and it
  costs nothing more per attempt file. An attempt file rewritten in place leaves its directory as
  it is and is not stat-ed again under the lock, so it is seen only if the writer told the phase,
  and otherwise not at any length or time; nothing in bead rewrites one in place, and nothing in
  the window writes at all.
- An identity taken of an attempt file is kept only while the store lock is held, for the rest of
  that phase, and forgotten when bead writes that file (`Workspace::forget_claims`); a reader
  outside the lock stats each file afresh, after the segments it reads (X13, roll-lqjj.30).
- Settings arrive as values (`Settings`); this crate never reads the process environment or the
  working directory. A numeric `BEAD_*` setting that is not a number is a `CONFIG` error when
  the settings are built, where Python died with a traceback (`docs/REVIEWING.md` §2).
- The repository's top, common directory and format are asked through `Settings::fact` (one
  `rev-parse`, each answer matching its separate call), never one call a question; the
  default coordination ref is valid without `check-ref-format`. `git` is set before the first
  question: the answers are its environment's. Nothing that can change while bead runs is kept:
  `find_workspace` asks origin's URLs each time it runs (`Checkout::origin_urls`), and
  `capture_where` asks HEAD and its branch each time it runs (`head_and_branch`). Each transaction
  captures these under the store lock at its start, so events record HEAD as it is then.
- Failpoints reach code through `Settings` only, never the process environment; anything they cannot
  do as written (an unknown point, an action the point cannot take, a second spec for one point)
  is `CONFIG` when the settings are built.
- It depends on `beadroll-core` and `beadroll-git` only, of the workspace.

## Common commands

- `cargo test -p beadroll-store`
- `cargo clippy -p beadroll-store --all-targets -- -D warnings -D clippy::mod_module_files`
