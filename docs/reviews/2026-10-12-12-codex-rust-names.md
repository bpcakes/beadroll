Independent native Codex review — committed branch scope

Reviewed HEAD `8918f9104b46d35975d6996f641d71ebd7d20375` against base and merge-base `274d4ffb278cf85a3fdb47a87710a72b04a6f668`. The complete included diff comprises 276 files; no paths were excluded and working-tree changes were not included.

No substantive correctness, security, data-loss, concurrency, performance or runtime-regression findings. The implementation changes consistently rename APIs and callers; removal of the sync-local trimming wrapper delegates directly to the same core implementation it previously called. Integer/boolean representation, parsing logic, reference values, receipt serialization, protocol logic and golden output files are preserved.

One supporting documentation obligation remains:

- **D1 — P3: Preserve the provenance of the remaining parity fixtures.** Location: `docs/RUST-PORT.md:742–751`. R2 requires historical fixture provenance to survive in the port document. The revised paragraph records golden files, receipt signatures, JSON diagnostics, printability and seeded-random fixtures, but the same change removes the explicit capture provenance from `tests/cli/parity/actors.rs`, `parity/inputs.rs` and the `COUNTS` fixture in `parity/pending.rs`. Neither the new paragraph nor the remaining port document identifies those recorded expectations as outputs captured from the original implementation before cutover; the actor/input headers previously specifically named CPython 3.14. Readers retain the expected values but lose their documented source. The old mixed-fleet discussion establishes historical testing, not these fixtures' origin. Add the omitted fixture groups and their capture provenance to this paragraph. This is a documentation completeness issue only: fixture bytes and assertions remain unchanged, and no behavioral failure is claimed.

Requirement coverage:

| Requirement | Assessment | Evidence |
| --- | --- | --- |
| R1: Rename Python-era Rust APIs/helpers and callers | Satisfied | `IntegerField`; numeric parsing/digit helpers; text-byte trimming/casing; normalized path helpers; seeded-random module. Workspace compilation includes all targets and features. Searches found no stale affected API references outside intentional migration/history material and active interpreter tooling. |
| R2: Rewrite source/tests/guides/metadata; preserve necessary tooling and provenance | Mostly satisfied; D1 is unmet | All eight AGENTS.md guides and affected Cargo metadata describe current roles. Remaining Python references in Rust concern active interpreter tooling, environment controls, generated-table paths or preserved fixture data. Provenance is relocated for the principal fixture groups, with the omission above. |
| R3: Preserve behavior, parsing, fixtures and format 1 | Satisfied by static inspection; runtime validation limited | Function bodies preserve their logic through renames. `IntegerField` retains numeric comparison and boolean display/JSON behavior; parsing bodies and receipt fixture bytes are unchanged. No golden files or Cargo.lock changes. Full baseline and runtime suites were not run in this review. |
| R4: Update active references and retain merged main fixes | Satisfied | FINDINGS.md updates the renamed U03 and receipt tests and CI module; the hardening catalog updates the active CI reference. The pinned base is HEAD's second parent, and the diff against that base preserves its runtime fixes. |

Validation performed in `<checkout>` (local path redacted for repository hygiene):

- `git diff --check 274d4ffb..8918f910`: exit 0.
- `cargo fmt --all -- --check`: exit 0. Complete log: `/tmp/beadroll-review-_diye59x/codex-fmt.log`.
- `cargo check --workspace --all-targets --all-features --locked --target-dir /tmp/beadroll-review-_diye59x/codex-target`: exit 0; all workspace crates and targets checked, completed in 9.27 seconds, no compiler diagnostics. Complete log: `/tmp/beadroll-review-_diye59x/codex-check.log`.

No additional material test gap was identified. Existing numeric, path, output, receipt and seeded-stream coverage remains intact; this review does not claim those runtime tests passed. No open behavior or scope questions remain.

Scope integrity: initial and final captures are complete, issue-free, clean, with no submodules and matching fingerprint `5c071a1ccf461317c87c49e7fd1bf8d49bb4f2a47a40a2383012dd624a0af385`. No repository files, index, refs or tracker records were changed.
