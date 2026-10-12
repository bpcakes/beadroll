# beadroll-core crate guide

## Purpose

`crates/beadroll-core` is the pure core of bead (`docs/RUST-PORT.md` §2.1): Python-compatible JSON
and text semantics, event ids and times, event validation, journal settling, causal order, replay
into state, readiness and the other queries, issue views, Beads import planning and export, key
minting, and reading a published tree. pancake-studio links this crate alone. Phase 1
(`roll-k2wy.2`) built it, proven equal to Python's implementation by `cargo xtask diff` until the
cutover removed both (`python-0.1.0` has them).

## Key entrypoints

- `src/lib.rs`: the crate root, listing the modules.
- `src/json.rs` (`json/parse.rs`, `json/write.rs`, `json/int.rs`): `Value`, `parse`, `write`,
  `Format`; `json/surrogates.rs`: `Surrogates`, lone surrogates kept through a read and a write.
- `src/text.rs`: `repr`, `str_of`, `strip`, `b36`, and `surrogateescaped`, `repr_bytes`,
  `printed_text` and `json_ascii_bytes` for bytes `surrogateescape` decodes (command lines,
  paths).
- `src/event.rs`: `event_problem`, `payload_problem`, `write_problem`, `Field`; `src/op.rs`: `Op`,
  an event's operation with its typed payload, and `PyInt`.
- `src/journal.rs` (`journal/read.rs`): `read_segment`, `read_attempt`, `Journal`;
  `journal/event.rs`: `Event`, what replay reads of a record (its id, causes, writer, actor, place
  and typed operation) and the digest of its content, with the texts and values events repeat held
  once (`Shared`); `journal/content.rs`: the digest. `src/order.rs`: `causal_order`, and `Order`,
  the same by event index.
- `src/replay.rs` (`replay/apply.rs`, with `apply/claims.rs` and `apply/create.rs`): `replay`,
  which takes what each event does from its journal; `src/state.rs` (`state/issue.rs`,
  `state/claim.rs`, `state/seen.rs`): `State`, `Issue`, claims, `saw`; `state/small.rs`:
  `VecMap`, `IdMap`, `IdSet`, the vectors an issue keeps its registers and tags in, grown one
  entry at a time while short.
- `state/seen.rs`: what every event had seen, as one clock per event over chains (each machine's
  journal, and chains of the unsequenced events of attempt files): the ancestry index behind
  `saw`, in place of Python's per-event `seen` and `own_anc` bitsets.
- `src/query.rs`, `src/view.rs`, `src/export.rs`, `src/import.rs`, `src/published.rs`: queries,
  `brief`/`detail`/`line`, `export_records`, `read_records`/`plan_import`, `Tree`.
- `tests/core/`: the Python suite's tests of the core, ported under the naming rule of
  `docs/RUST-PORT.md` §4.2.

## Edit here for X

- Reading or writing JSON the way Python does (§2.3a, §5): `src/json/`.
- What an event may hold: `src/event.rs`, with its Python message. Format 1 is frozen: a change
  is additive, so that a bead that predates it, `python-0.1.0`'s included, sets the new thing
  aside rather than misreads it.
- Replay, state, readiness, cycles, views, import planning and export: the module of that name.
  Until the cutover every change was checked against Python's `model.py`, `queries.py` and
  `commands.py` by `cargo xtask diff`; the tests in `tests/core/` and the black-box tests of the
  `beadroll` crate are what holds it now.
- Anything that reads a file, starts a process, or asks the clock or the environment: not here,
  but `beadroll-store`, `beadroll-git`, or the CLI, which passes the value down.

## Invariants

- No I/O: no filesystem, processes, clock or environment (§2.2).
- It depends on no other workspace crate; any of them may depend on it.
- No panics: `unwrap`, `expect`, `panic!`, `todo!` and `unimplemented!` are denied, and replay
  is total: every input is applied or reported.
- No `HashMap` or `HashSet` iteration order reaches an output; issues keep replay order.
- Its answers are Python's, proven differentially against CPython until the cutover (`xtask
  diff`, §4.2); a departure is a decided row in §5, never a drift.
- `saw` answers what Python's bitsets answer: `state/seen/tests.rs` checks every pair against
  them on hostile histories. Replay's speed and memory are measured by
  `cargo run --release --package xtask -- timing --replay N [--ownership]`: at 200,000 events its
  heap peak is within the 150 MB of `docs/RUST-PORT.md` Appendix A (roll-dih8.11). Replay keeps
  no event's parsed JSON: a record is read into an `Event` once, and the state keeps what `saw`
  and `who` read of each event, not the journal.

## Common commands

- `cargo test -p beadroll-core` (unit tests and `tests/core/`)
- `cargo clippy -p beadroll-core --all-targets -- -D warnings -D clippy::mod_module_files`
