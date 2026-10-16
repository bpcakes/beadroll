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
  `projection/codec.rs`, `projection/state_codec.rs`, `state/seen/codec.rs`: its bytes. Its
  state part (`decode_state`, what a command that only reads decodes) holds of its events only who
  wrote those behind a register with several entries and where journals broke; its index holds
  every event the state names with its causes, machine and actor (roll-lqjj.35). Its index also
  holds what `bead info` and `bead machines` answer from (roll-lqjj.31): the frontier,
  every event id (`Projection::event_count`), and each machine's journal length and latest event
  (`Projection::journals`), which `project` takes from the journal's order and `extend` advances.
- `state/seen.rs`: what every event had seen, as one clock per event over chains (each machine's
  journal, and chains of the unsequenced events of attempt files): the ancestry index behind
  `saw`; its tests compare it with per-event ancestry bitsets.
- `src/query.rs`, `src/view.rs`, `src/export.rs`, `src/import.rs`, `src/published.rs`: queries,
  `brief`/`detail`/`line`, `export_records`, `read_records`/`plan_import`, `Tree`.
  `src/query/readiness.rs`: `State::readiness`, whether anything holds each issue, judged once for
  all of them (`Readiness`), which `State::ready` and the CLI's `ready`, `blocked` and `prime` ask,
  and `State::is_blocked`, the same question for one issue, answered by the first thing found
  (roll-lqjj.19); `State::open_blockers` lists one issue's blockers themselves, and
  `src/query/blockers.rs`: `State::blocker_lists`, every issue's list at once (`BlockerLists`),
  equal for every issue to `open_blockers`, which `bead blocked` prints (roll-lqjj.24).
  `src/view/dependents.rs`: `State::dependents_of`, the issues that depend on each of several
  issues, found in one pass over every issue's edges (`Dependents`), from which
  `State::detail_with` lists each one's; `State::detail` makes it for its one issue (roll-lqjj.20).
  `src/view/details.rs`: `State::details_of`, what the details of several issues share, their
  dependents and, for several, `State::readiness` for their `blocked` (`Details`), which `bead
  show`, `update`, `claim` and `takeover` make their details from (roll-lqjj.24).
- `src/import/dry_run.rs`: `problems` (every problem an import would be refused for, empty exactly
  when it goes through) and `preview` (what it would bring), for `--dry-run`.
- `src/work.rs`: `tally`, the work the algorithms of roll-lqjj.15 to .20 do, counted in this crate's
  own tests alone (`cfg(test)`; it does nothing in any other build), and `work/tests/`, the guards
  that fail when that work grows with the square of its input (roll-lqjj.34, X15), among them the
  blockers and details of several issues (X01, X02) and the keys a renamed copy asks of (X03,
  roll-lqjj.38).
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
- Every field a register holds is kept there once and shown from there by `Issue::field`: a long
  field's (roll-lqjj.10) and a short one's (roll-lqjj.41), the entry of the largest id, a null shown
  as absent unless the create wrote it, which a long field shows while that entry is the largest and
  a short field only while it is the register's one entry; status, closed_at and close_reason from
  the members of the status register's entry, null members included, unless a set wrote closed_at or
  close_reason after the status into a register of its own, which `Stamps` records. An issue keeps
  beside its registers only what no register holds (`Stamps`: created_at, created_by, updated_at),
  and a title, priority or issue_type a create did not give shows its default (`""`, 2, `task`)
  until a write gives it a register; replay copies nothing out of a register.
  `projection/tests/shown.rs` compares every field of every issue with a model of what replay showed
  when it copied values out, on every history the differential tests replay (their creates and sets
  draw defaults, nulls, legacy boolean priorities, closed creates, imports' stamps, and status,
  closed_at and close_reason in either order; updated_at is checked there only as one of the issue's
  events' times) and exactly on two journals bead built, and `tests/core/replay/short_fields.rs`
  each case where a create, a later write and that copy differ, among them a write that had not seen
  the create and has the smaller id, which no history makes.
- A state read alone (`projection::decode_state`, which a read decodes when it finds the
  projection current; a write, the first read after one, `info` and `machines` decode the whole
  with `Projection::decode`) knows of
  its events only who wrote those behind a register with several entries, the only events
  `State::conflicts` names, and where machines' journals broke; every event the state names, with
  its causes, machine and actor, is in the projection's index, which only `Projection::decode`
  reads (roll-lqjj.35, X16). Anything else asked of an event (`State::who` of another, `saw`,
  `holds`, `events`) is asked of a projection or of replay; `read_back` in `projection/tests.rs`
  checks both parts on every history the differential tests read back.
- A projection extended is replay: `projection/tests.rs` checks every batch a store takes in on
  histories of machines that write, sync and race, and each batch it must decline. What replay
  reports does not stop it (roll-lqjj.28): `projection/tests/flawed.rs` runs the same on histories
  holding every kind of integrity entry, a copy of a machine's identity among the writers, and
  `projection/tests/reported.rs` each kind by hand. It is not extended when an event waits for a
  cause, or when a record still to come can change what replay made of the events before it
  (`replay::Replayed::unsettled`: a takeover of an id no event has, an id with several contents
  placed by less than its causes, `Order::loose`), and it declines a batch with a record of an id
  an integrity entry names; anything added to what replay reports must say which it is, and
  `cargo test --release --package beadroll-core -- --ignored many_seeds` runs 10,000 histories.
  What the finishing pass derives (each issue's labels, the parent-child index) is redone for what
  the batch can have changed alone, so an operation added to `Op` must say in `replay::relabels`
  and `replay::rewires` whether it changes labels or an edge; the compiler asks, and
  `projection/tests/finished.rs` keeps each answer against replay (roll-lqjj.17, `docs/REVIEWING.md`
  §2). An issue is relabelled once a batch, not once an event, and the ids a projection holds are a
  `BTreeSet` so a batch's go in wherever they sort among them; the bytes stay the ascending
  sequence they were, and bytes that are not ascending are still refused.
- A create visits only the imported dependencies that wait for its key, and a removal by key only
  an issue's entries for that key: `State::awaiting` lists each by the key it names, its issue and
  a number it keeps, its place when its list began to wait (roll-lqjj.18). An entry taken out
  leaves null in its place while events apply, and the finishing pass (`finish`, `finish_some`)
  makes each such list whole, so no state outside replay holds a null, and lists nothing again
  (W01). The index is derived and never written: a state read back builds it from the issues'
  lists, and `projection/tests/awaiting.rs` keeps replay, a projection extended and one read back
  listing the same entries at the same places, and what replay made of waiting imports before it.
- A copy renamed for a key two machines minted is given its key by asking of each candidate
  (`State::claims_key`: `by_key` holds it, or `Awaiting::names` finds an entry that waits for it),
  never by collecting `claimed_keys()` for each copy (roll-lqjj.25, X03); `claimed_keys` is public
  and keeps its answer, and `mint_key` still collects it, once a command. Under test every
  `claims_key` answer is checked against `claimed_keys` and kept (`state/asked.rs`), so replay and
  projection tests that rename copies prove the two agree.
- Readiness is one rule, the one `State::open_blockers` lists blockers by, and whether anything
  holds an issue is answered three ways that must agree: judged for every issue at once
  (`State::readiness`, each issue, edge and child visited once, a merged cycle once, nothing
  recursing), for one issue by the first thing found (`State::is_blocked`), and by the list itself,
  which a reader that lists many issues' blockers makes for all of them at once
  (`State::blocker_lists`, equal for every issue to `open_blockers`, roll-lqjj.24);
  `tests/core/replay/readiness.rs` keeps them agreeing on deep chains, lattices, shared and second
  parents, merged parent cycles, done and disputed ancestors and targets, epics beside plain
  parents, claims, scopes and random graphs (roll-lqjj.19). A reader that asks of many issues asks
  `readiness` or `blocker_lists` once: asked issue by issue, a chain of N issues costs N². `cargo
  run --release --package xtask -- timing --ready N` measures the ways.
- A detail's `dependents` are every issue with an edge to its issue's uid, in replay order, each
  one's edges to it in sorted order, never an edge that was removed; only the edges say so, so
  finding them reads every issue's edges. A reader that shows several issues in full
  finds them for all of them in one pass (`State::dependents_of`) and makes each detail with
  `State::detail_with`: asked issue by issue, N details read every edge N times (roll-lqjj.20). A
  detail's `blocked` is judged the same way: `State::details_of` finds the dependents and, for
  several issues, readiness once for all of them, and `Details::of` makes each detail; `State::detail`
  of one issue walks its own ancestors (roll-lqjj.24).
  `tests/core/replay/dependents.rs` keeps the details so made equal to `State::detail` of each
  issue alone and to what every detail found before, member for member, and `cargo run --release
  --package xtask -- timing --details N` measures both ways.
- What those algorithms do grows with their input, and `work/tests/` keeps it so: each counts its
  work on the shape it was quadratic on, at a size and at eight times it, and fails above sixteen
  times as much (roll-lqjj.34, X15). The counts are kept where every way of doing the work passes
  (`work::tally`): an issue's edges when `Issue::deps` or `Issue::dep_tags` hands them out and a
  parent's children when `State::children` does, an issue relabelled (`replay::relabel`), the
  parent-child index built (`State::index_children`), a waiting import's key read
  (`state::awaited`, which replay reads it through and nowhere else) and the entries `Awaiting`
  hands out, examines or makes whole, an import's record or dependency key read (`id_of`,
  `target`), and the keys `State::claims_key` is asked of and `State::claimed_keys` collects. Read
  an edge, a waiting key or an import key through those, never around them, and collect the
  claimed keys through `claimed_keys`, or a guard stops seeing the work. `blocker_lists` counts the
  rows it walks (`Rows::row`, `Rows::item`), and the test-only check that each `claims_key` ask
  agrees with the keys claimed (`state/asked.rs`) reads them directly, so that it counts nothing.
  What a count cannot see, a lookup turned back into a scan, is timed by `xtask`'s growth tests,
  nightly in release.
- `saw` agrees with the reference ancestry bitsets: `state/seen/tests.rs` checks every pair
  on hostile histories. Replay's speed and memory are measured by
  `cargo run --release --package xtask -- timing --replay N [--ownership]`: at 200,000 events its
  heap peak is within its 150 MB budget (`docs/REVIEWING.md` §2; roll-dih8.11). Replay keeps
  no event's parsed JSON: a record is read into an `Event` once, and the state keeps what `saw`
  and `who` read of each event, not the journal.

## Common commands

- `cargo test -p beadroll-core` (unit tests and `tests/core/`)
- `cargo clippy -p beadroll-core --all-targets -- -D warnings -D clippy::mod_module_files`
