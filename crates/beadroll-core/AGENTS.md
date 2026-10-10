# beadroll-core crate guide

## Purpose

`crates/beadroll-core` is the pure core of bead (`docs/RUST-PORT.md` §2.1): Python-compatible JSON
and text semantics, event ids and times, event validation, journal settling, causal order, replay
into state, readiness and the other queries, issue views, Beads import planning and export, key
minting, and reading a published tree. pancake-studio links this crate alone. Phase 1
(`roll-k2wy.2`) built it, proven equal to Python's implementation by `cargo xtask diff`.

## Key entrypoints

- `src/lib.rs`: the crate root, listing the modules.
- `src/json.rs` (`json/parse.rs`, `json/write.rs`, `json/int.rs`): `Value`, `parse`, `write`,
  `Format`; `json/surrogates.rs`: `Surrogates`, lone surrogates kept through a read and a write.
- `src/text.rs`: `repr`, `str_of`, `strip`, `b36`, and `surrogateescaped`, `repr_bytes`,
  `printed_text` and `json_ascii_bytes` for bytes `surrogateescape` decodes (command lines,
  paths).
- `src/event.rs`: `event_problem`, `payload_problem`, `write_problem`, `Field`; `src/op.rs`: `Op`,
  an event's operation with its typed payload, and `PyInt`.
- `src/journal.rs`: `read_segment`, `read_attempt`, `Journal`; `src/order.rs`: `causal_order`.
- `src/replay.rs` (`replay/apply.rs`): `replay`; `src/state.rs` (`state/issue.rs`,
  `state/seen.rs`): `State`, `Issue`, claims, `saw`.
- `src/query.rs`, `src/view.rs`, `src/export.rs`, `src/import.rs`, `src/published.rs`: queries,
  `brief`/`detail`/`line`, `export_records`, `read_records`/`plan_import`, `Tree`.
- `tests/core/`: the Python suite's tests of the core, under the inventory's naming rule.

## Edit here for X

- Reading or writing JSON the way Python does (§2.3a, §5): `src/json/`.
- What an event may hold: `src/event.rs`, with its Python message, and Python's `payload_problem`
  changed the same way.
- Replay, state, readiness, cycles, views, import planning and export: the module of that name;
  every change is checked with `cargo xtask diff replay` against `beadroll/model.py`,
  `queries.py` and `commands.py`.
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

- `cargo test -p beadroll-core` (unit tests and `tests/core/`)
- `cargo xtask diff replay` and `cargo xtask diff json` (against CPython; `--cases N`, `--seed S`)
- `cargo clippy -p beadroll-core --all-targets -- -D warnings -D clippy::mod_module_files`
