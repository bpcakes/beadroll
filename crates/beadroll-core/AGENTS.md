# beadroll-core crate guide

## Purpose

`crates/beadroll-core` is the pure core of bead: strict JSON and Unicode text semantics, event ids
and times, event validation, journal settling, causal order, replay into state, readiness and the
other queries, issue views, Beads import planning and export, key minting, and reading a published
tree. pancake-studio links this crate alone, as a git dependency pinned to a commit: it reads a
coordination ref's tree with `published::Tree::read(entries, expected_workspace)`, which refuses
a tree without a readable manifest, of another workspace or in another format, replays
`tree.journal()` with `replay::replay`, and takes `State::export_records` for its Beads reader.
A `State` is immutable and `Send + Sync`, to share behind an `Arc`; nothing is async, so a
consumer on tokio calls through `spawn_blocking`.

## Key entrypoints

- `src/lib.rs`: the crate root, listing the modules.
- `src/json.rs` (`json/parse.rs`, `json/write.rs`, `json/int.rs`): `Value`, `parse`, `write`,
  `Format`; `json/surrogates.rs`: `Surrogates`, lone surrogates kept through a read and a write.
- `src/text.rs`: `repr`, `str_of`, `strip`, `b36`, and `surrogateescaped`, `repr_bytes`,
  `printed_text` and `json_ascii_bytes` for bytes `surrogateescape` decodes (command lines,
  paths).
- `src/event.rs`: `event_problem`, `payload_problem`, `write_problem`, `Field`; `src/op.rs`: `Op`,
  an event's operation with its typed payload, and `IntegerField`.
- `src/journal.rs` (`journal/read.rs`): `read_segment`, `read_attempt`, `Journal`;
  `journal/event.rs`: `Event`, what replay reads of a record (its id, causes, writer, actor, place
  and typed operation) and the digest of its content, with the texts and values events repeat held
  once (`Shared`); `journal/content.rs`: the digest. `src/order.rs`: `causal_order`, and `Order`,
  the same by event index.
- `src/replay.rs` (`replay/apply.rs`, with `apply/claims.rs` and `apply/create.rs`): `replay`,
  which takes what each event does from its journal; `src/state.rs` (`state/issue.rs`,
  `state/claim.rs`, `state/seen.rs`): `State`, `Issue`, claims, `saw`; `state/small.rs`:
  `VecMap`, `IdMap`, `IdSet`, the vectors an issue keeps its registers and tags in, grown one
  entry at a time while short; `state/awaiting.rs`: `Awaiting`, where every imported dependency
  that waits for a key is, by that key (roll-lqjj.18).
- `src/projection.rs`: `project`, `Projection` (roll-lqjj.2), a replayed state kept to have events
  applied after it: what every event had seen cut to the events the state still names
  (`state/seen/compact.rs`); `projection/extend.rs`: `Projection::extend`, which applies records
  appended to its journal when replay would apply them after all of it, and declines otherwise, and
  then finishes only what the batch changed (`replay::finish_some`, roll-lqjj.17);
  `projection/codec.rs`, `projection/state_codec.rs`, `state/seen/codec.rs`: its bytes.
- `state/seen.rs`: what every event had seen, as one clock per event over chains (each machine's
  journal, and chains of the unsequenced events of attempt files): the ancestry index behind
  `saw`; its tests compare it with per-event ancestry bitsets.
- `src/query.rs`, `src/view.rs`, `src/export.rs`, `src/import.rs`, `src/published.rs`: queries,
  `brief`/`detail`/`line`, `export_records`, `read_records`/`plan_import`, `Tree`.
- `src/import/dry_run.rs`: `problems` (every problem an import would be refused for, empty exactly
  when it goes through) and `preview` (what it would bring), for `--dry-run`.
- `tests/core/`: integration tests of replay, storage compatibility and validation.

## Edit here for X

- Reading or writing JSON with the established format and diagnostics (`docs/REVIEWING.md` §2):
  `src/json/`.
- What an event may hold: `src/event.rs`, including its validation message. Format 1 is frozen:
  changes are additive, so older versions set unfamiliar records aside rather than misread them.
- Replay, state, readiness, cycles, views, import planning and export: the module of that name.
  A change to what replay keeps in a `State` or an `Issue` changes a projection's bytes: write and
  read it in `projection/state_codec.rs`, and bump `projection::ENCODING`.
  Tests in `tests/core/` and the black-box tests of the `beadroll` crate keep these contracts.
- Anything that reads a file, starts a process, or asks the clock or the environment: not here,
  but `beadroll-store`, `beadroll-git`, or the CLI, which passes the value down.

## Invariants

- No I/O: no filesystem, processes, clock or environment.
- It depends on no other workspace crate; any of them may depend on it.
- No panics: `unwrap`, `expect`, `panic!`, `todo!` and `unimplemented!` are denied, and replay
  is total: every input is applied or reported.
- No `HashMap` or `HashSet` iteration order reaches an output; issues keep replay order.
- Parsing, rendering and replay preserve the compatibility contracts in `docs/REVIEWING.md` §2;
  a departure requires a documented decision and regression coverage.
- A long field's value is kept once, in its register; `Issue::field` shows it from there, so
  replay never copies it into the issue's fields (roll-lqjj.10).
- A projection extended is replay: `projection/tests.rs` checks every batch a store takes in on
  histories of machines that write, sync and race, and each batch it must decline.
  What the finishing pass derives (each issue's labels, the parent-child index) is redone for what
  the batch can have changed alone, so an operation added to `Op` must say in `replay::relabels`
  and `replay::rewires` whether it changes labels or an edge; the compiler asks, and
  `projection/tests/finished.rs` keeps each answer against replay (roll-lqjj.17, `docs/REVIEWING.md`
  §2). An issue is relabelled once a batch, not once an event, and the ids a projection holds are a
  `BTreeSet` so a batch's go in wherever they sort among them; the bytes stay the ascending
  sequence they were, and bytes that are not ascending are still refused.
- A create visits only the imported dependencies that wait for its key, and a removal by key only
  an issue's entries for that key: `State::awaiting` lists each by the key it names, its issue and
  its place in the issue's list (roll-lqjj.18). An entry taken out leaves null in its place while
  events apply, and the finishing pass (`finish`, `finish_some`) makes each such list whole, so no
  state outside replay holds a null. The index is derived and never written: a state read back
  builds it from the issues' lists, and `projection/tests/awaiting.rs` keeps replay, a projection
  extended and one read back agreeing on it, and on what replay made of waiting imports before it.
- `saw` agrees with the reference ancestry bitsets: `state/seen/tests.rs` checks every pair
  on hostile histories. Replay's speed and memory are measured by
  `cargo run --release --package xtask -- timing --replay N [--ownership]`: at 200,000 events its
  heap peak is within its 150 MB budget (`docs/REVIEWING.md` §2; roll-dih8.11). Replay keeps
  no event's parsed JSON: a record is read into an `Event` once, and the state keeps what `saw`
  and `who` read of each event, not the journal.

## Common commands

- `cargo test -p beadroll-core` (unit tests and `tests/core/`)
- `cargo clippy -p beadroll-core --all-targets -- -D warnings -D clippy::mod_module_files`
