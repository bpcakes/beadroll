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

- `src/metadata.rs`: `Metadata`, the typed published-count map with ordered unknown fields kept
  for rewrites; `src/configuration.rs`: known configuration fields read by type into the existing
  public map, with legacy shapes left to the established validators.

- `src/lib.rs`: the crate root, listing the modules.
- `src/json.rs` (`json/read.rs`, `json/write.rs`, `json/int.rs`): `Value` and its `Deserialize`,
  `parse` (serde_json), `write`, `Format`.
- `src/text.rs`: `repr`, `str_of`, `strip`, `b36`, and `surrogateescaped`, `repr_bytes`,
  `printed_text` and `json_ascii_bytes` for bytes `surrogateescape` decodes (command lines,
  paths).
- `src/event.rs`: `event_problem`, `payload_problem`, `write_problem`, `Field`;
  `event/envelope.rs`: `Envelope`, an event's envelope as read, which a journal line is read
  straight into by serde_json and a value built in memory walks into, and whose rules
  `event_problem` judges (roll-lqjj.53.1); `src/op.rs`: `Op`, an event's operation with its typed
  payload, read out of the payload's object, and `IntegerField`.
- `src/journal.rs` (`journal/read.rs`): `Journal`, with `add_segment` and `add_attempt`, which
  read a file's lines straight into what the journal keeps, and `add` for a record built in
  memory; `read_segment` and `read_attempt`, the same files read into values for a tool that
  needs the records as written; `journal/event.rs`: `Event`, what replay reads of a record (its
  id, causes, writer, actor, place and typed operation) and the digest of its content, with the
  texts and values events repeat held once (`Shared`); `journal/content.rs`: the digest, of the
  line's bytes; `journal/tests.rs`: a journal read from lines is the journal of the values read
  from them, on the differential tests' histories. `src/order.rs`: `causal_order`, and `Order`,
  the same by event index. `src/recap.rs`: `unseen`, what `bead recap` shows of a journal below
  the frontier it kept, the frontier it keeps then and the events its read placed, and `joined`,
  a frontier with the ids of another that its read did not place (roll-dih8.2); its tests deliver
  the differential tests' histories to a growing store, and to two reads that saw different parts
  of one.
- `src/replay.rs` (`replay/apply.rs`, with `apply/claims.rs` and `apply/create.rs`): `replay`,
  which takes what each event does from its journal; `src/state.rs` (`state/issue.rs`,
  `state/claim.rs`, `state/seen.rs`): `State`, `Issue`, claims, `saw`; `state/small.rs`:
  `VecMap`, `IdMap`, `IdSet`, the vectors an issue keeps its registers and tags in, grown one
  entry at a time while short; `state/awaiting.rs`: `Awaiting`, where every imported dependency
  that waits for a key is, by that key (roll-lqjj.18); `state/edges.rs`: `Edge` (a `Target`, an
  issue's id or the text of one that is no id, and its kind), `Meta`, the metadata an addition of
  an edge carries, and `Deps`, what `Issue::deps` hands out (roll-lqjj.39); `state/edges/tags.rs`:
  `DepTags`, an issue's edges and the additions behind each, a vector searched in order while they
  are few and indexed by target besides once they are many (roll-lqjj.21); `state/issues.rs`:
  `Issues`, a state's issues in replay order and which of those read back from a projection
  changed since (roll-lqjj.44); `state/runs.rs`: `Runs`, the runs of numbers renamed copies found
  claimed under each stem, which the next copy of a numbered key skips (roll-lqjj.46).
- `src/projection.rs`: `project`, `Projection` (roll-lqjj.2), a replayed state kept to have events
  applied after it: what every event had seen cut to the events the state still names
  (`state/seen/compact.rs`), by `project` and then now and then, by the write-back that finds the
  events taken in since more than a `COMPACTED`th of those it knew when last cut
  (`Projection::compaction_due`, roll-lqjj.43); `projection/extend.rs`: `Projection::extend`, which applies records
  appended to its journal when replay would apply them after all of it, and declines otherwise, and
  then finishes only what the batch changed (`replay::finish_some`, roll-lqjj.17);
  `projection/codec.rs`, `projection/state_codec.rs`, `state/seen/codec.rs`: its bytes, two parts
  of records each referring to the names and values they share by number in a table of the part's
  own (`codec::Table`); `projection/parts.rs`: `Projection::encode` and `Projection::decode`,
  which keeps the bytes it read (`Kept`) until `Projection::let_go`, so that a write-back copies
  the record of every issue and event that did not change since (roll-lqjj.44). Its state part
  (`decode_state`, what a command that only reads decodes) holds of its events only who wrote
  those behind a register with several entries, in each issue's record, and where journals broke;
  its index holds the record of every event the state names, its causes, machine, actor, place and
  clock (roll-lqjj.35), and between compactions every event taken in since, with how many it knew
  when last compacted. Its index also
  holds what `bead info` and `bead machines` answer from (roll-lqjj.31): the frontier,
  every event id (`Projection::event_count`), and each machine's journal length and latest event
  (`Projection::journals`), which `project` takes from the journal's order and `extend` advances.
  `projection/tests/owned.rs`: what every issue answers of who held it, pinned before its
  ownership history moved to its texts (roll-lqjj.40), and the generator's ownership draws
  (`World::own`, `World::race`).
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
- `src/work.rs`: `tally`, the work the algorithms of roll-lqjj.15 to .21 do, counted in this crate's
  own tests alone (`cfg(test)`; it does nothing in any other build), and `work/tests/`, the guards
  that fail when that work grows with the square of its input (roll-lqjj.34, X15), among them the
  blockers and details of several issues (X01, X02), the keys a renamed copy asks of (X03,
  roll-lqjj.38), of numbered keys too (roll-lqjj.46), and the edges looked at to give one issue
  many (`degree`, roll-lqjj.21).
- `tests/core/`: integration tests of replay, storage compatibility and validation;
  `tests/core/compat/json_round_trip.rs`: JSON round-trips in every dialect bead writes, on seeded
  values (roll-k2wy.14.4).
- `src/replay/tests.rs`: replay is invariant under delivery order, on histories of the
  differential tests' generator (`projection::tests::history`, roll-k2wy.14.4).

## Edit here for X

- Reading JSON (serde_json into `Value`) or writing it in the established formats
  (`docs/REVIEWING.md` §2): `src/json/`.
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
  `holds`, `events`) is asked of a projection or of replay, and a state read alone lists its
  writers in issue order, not the journal's; `read_back` in `projection/tests.rs`
  checks both parts on every history the differential tests read back. Between compactions the
  index also holds events the state no longer names (roll-lqjj.43): nothing a command answers or
  decides may read them, and `holds` and `events` of a projection's state list them. Every track of
  the differential tests is kept three ways (`WriteBack`): as the store writes it back, and at every
  write-back uncompacted or compacted, each compared with replay, who wrote every event the state
  names and what each head had seen of it included (`answers_alike`).
- A write-back copies what did not change since the projection was read back (roll-lqjj.44): the
  record of every issue `Issues::changed` does not name, the event records read back, which
  between two compactions never change and are only added after, and the chain sets and positions
  their clocks share; it writes everything else. So every change to an issue goes through `Issues`,
  which marks it (`get_mut`, `values_mut`, an `insert` of an id already there), and nothing
  removes or moves an issue; only reading its texts in (`read_in`, for `State::read_texts`) and
  deriving its labels (`relabel`) leave it unmarked, since they change nothing written, and an issue
  whose texts are to be placed anew counts as changed whatever its marks. A record's bytes depend on
  nothing outside it but its part's table, which only grows until it is made anew (the state part's
  past an eighth more entries, the index's at a compaction), and its texts' place. Since records
  are copied by position, decode refuses a state part that holds an issue or a key twice, and a
  count of the entries made anew beyond its table's (`projection/tests/forged.rs`).
  `projection/tests/marked.rs` checks at every extension of every projection the differential
  tests read back that every issue whose record changed is marked, that nothing else is but issues
  the batch is about or that wait for a key it brings, and that every event read back keeps its
  record; `read_back` checks that what a write-back copied is byte for byte what it writes when it
  copies nothing; `projection/tests/copied.rs` the ids and the table the histories do not reach.
- An issue's edges are typed (roll-lqjj.39): a target is `Target::Id` exactly when its text is an
  id's spelling (`Target::of`; an id that is no issue's stays an id), and text otherwise, shown and
  ordered by that text, so edges sort as the pairs of strings did; a kind is one `Arc<str>` for
  every edge of it in a state replayed or decoded, through replay's `Names` and the projection's
  name table (an extension interns its batch's alone, so an edge it adds holds its own until the
  next decode). The state part keeps each edge and the events that added it; the metadata of each
  addition is with the issue's texts, `None` in a state read alone until they are read in, which
  only an export (and applying more events) needs. So an edge added or removed changes an issue's texts, and so does a create of a
  key another issue's import waits for, which makes that issue an edge: `Projection::extend`
  declines when such an issue's texts are not in, and `Projection::about` lists them for a store to
  read in first. Read an edge through `Issue::deps` and `Issue::dep_tags`, never the field; add an
  edge through `DepTags::added` alone, and change an edge's additions through it, or through
  `DepTags::for_each_to` or `DepTags::values_mut`, which never add or move an edge (roll-lqjj.21):
  an issue's edges are a vector, in the order first added, searched in order while there are at most
  `state::edges::tags::LISTED` (16), and from the first change past that also indexed, a table of
  their places hashed by target, which finds an edge, or every edge on a target, looking at the
  edges on that target alone. The index is derived and never written: a projection's bytes are what
  they were, a state read back with more than 16 edges on an issue holds them unindexed until the
  thirteenth change to them (`SEARCHES`), so a command, which changes an issue a few times, builds
  none, and equality and `Debug` see the edges alone. Core's own tests index an issue's edges at a
  change once it has two, and those read back at their second change (`LISTED` and `SEARCHES` are 1
  under `cfg(test)`), so every differential history takes the indexed path where its issues have a
  few edges, and `state/edges/tags/tests.rs` compares it with the vector searched in order on seeded
  draws; the integration tests, which build the crate as bead does, take the bound bead runs with
  (`tests/core/replay/many_edges.rs`). `projection/tests/edges.rs` pins what
  every issue's edges answered when they were held as strings (edges, tags and metadata in order,
  readiness, blockers, dependents, cycles, details and the export, as digests of every state the
  differential histories pass through and in full on three bead-built journals), and checks export
  and show alike with the metadata read from the texts; `projection/tests/edges/drawn.rs` is what
  the differential generator draws for it.
- An issue's ownership history (the claims that ended, the claims that lost, the ends that came
  too late) is kept with its texts (roll-lqjj.40): a state read alone has it empty until they are
  read in, as every command that prints an issue whole does (`show`, `update`, `claim`,
  `takeover`, `comments`, `export`) and as the store does for the issues a batch is about before
  `Projection::extend` applies it (`Projection::about`). Its claim, its generation and its offline claims, which every
  ownership decision reads, stay in the state part, and so does who ended its last ended claim and
  how (`Issue::last_ended`, from `Stored`), which `close`, `delete` and `update` ask of issues whose
  texts they do not read in. Replay adds to the history only through `Issue::ended`, `Issue::lost`
  and `Issue::ignored`, which have texts read in written again; `extend` declines a claim, release,
  close, reopen or delete of an issue whose texts are not read in. `projection/tests/owned.rs` pins
  what every issue answered of its ownership before the change, on three journals bead built and
  on every differential and delivery-order history, and `read_back` checks a state read back with
  some issues' texts read in alone.
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
- A copy of a numbered key skips the runs of numbers earlier copies found claimed (`State::runs`,
  `state/runs.rs`, roll-lqjj.46): it takes `key::free_key`'s key, the first number at or above its
  own whose key nothing claims, but asks only of the number after each run it meets, so the N copies
  two machines make of one parent's children ask of about 3N keys rather than N². Every number in a
  run is claimed. A key stops being claimed only when an import that waited for it is taken back by
  key (`Awaiting::retract`), and `dep_remove` then takes it out of its run (`Runs::forget`); any new
  way to take an entry out of `Awaiting` must do the same. The runs are derived and never written,
  so a state read back starts with none. Under test every key `State::free_key` picks is checked
  against `key::free_key` asked one number at a time (`runs::agrees`, whole up to 512 numbers), and
  `projection/tests/numbered.rs` keeps replay, a projection extended and one read back picking what
  replay picked before the runs.
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
  `target`), the keys `State::claims_key` is asked of and `State::claimed_keys` collects, each run
  `state::Runs` looks up, extends or splits, and each
  edge `DepTags` compares, each search of its index and each edge an index is built of
  (`Work::Probe`). Read an edge, a waiting key or an import key through those, never around them,
  and collect the claimed keys through `claimed_keys`, or a guard stops seeing the work.
  `blocker_lists` counts the rows it walks (`Rows::row`, `Rows::item`), and the test-only check that
  each `claims_key` ask agrees with the keys claimed (`state/asked.rs`) reads them directly, so that
  it counts nothing. What a count cannot see, a lookup turned back into a scan, is timed by
  `xtask`'s growth tests, nightly in release.
- Replay answers alike whatever order a journal is given its records in, as long as each
  journal keeps its own: `replay::tests` delivers each generated history in random causal orders,
  random interleavings of its journals and a store's order, and compares every answer the public
  API gives (every `Issue::field`, the views, export, readiness, blockers, cycles, conflicts, what
  replay reports, who wrote each event and what it saw); with histories holding what replay
  reports, it keeps each journal's order and compares those reports as a set, since they are
  listed in the order found. Its histories cost about 0.9 s and 0.7 s of the fast suite. An id
  with several contents and not exactly one copy of its own is placed by the copy found first,
  and with it the issue it names and all that issue answers (roll-k2wy.14.14). Every `Format`
  writes what `json::parse` reads back as the same value and writes again as the same text
  (`compat::json_round_trip`), and `fuzz/` fuzzes the segment reader and the importer
  nightly (`fuzz/README.md`).
- `saw` agrees with the reference ancestry bitsets: `state/seen/tests.rs` checks every pair
  on hostile histories. Replay's speed and memory are measured by
  `cargo run --release --package xtask -- timing --replay N [--ownership]`: at 200,000 events its
  heap peak is within its 150 MB budget (`docs/REVIEWING.md` §2; roll-dih8.11). Replay keeps
  no event's parsed JSON: a record is read into an `Event` once, and the state keeps what `saw`
  and `who` read of each event, not the journal.

## Common commands

- `cargo test -p beadroll-core` (unit tests and `tests/core/`)
- `cargo clippy -p beadroll-core --all-targets -- -D warnings -D clippy::mod_module_files`
