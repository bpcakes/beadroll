# Review 8: Rust port epic and plan

2026-10-09. Scope: `roll-k2wy` and its 16 children, `docs/RUST-PORT.md`, the three
catalogues in `docs/rust-port/`, and the implementation and consumer boundaries they reference.
Plan commit: `492b91633264bd0778ae19d612540414771fbf2f`; its Python baseline is `9867715`.
Studio source inspected at `/Users/<name>/Documents/pancake-studio`, HEAD
`6aa093f0c7efdb7b33f32a059fc52f842bb3b7c9`.

The plan is not ready to execute unchanged. There are six material findings: one high and five
medium. The overall decomposition and test inventory are sound; the defects concern compatibility,
specific implementation instructions, and cutover coverage. This is a plan audit, not a claim that
a Rust implementation already contains these bugs.

The maintainer's five decisions in §10 are accepted. None of the findings asks to change the
repository, release channels, MSRV, always-available failpoints, or the two approved input rejections.

## Findings

### S01 — High: syncing does not make incompatible receipts safe to upgrade

**Plan:** §1.4, §5 row 21 (`docs/RUST-PORT.md:845`), and §8's upgrade procedure
(`docs/RUST-PORT.md:1003`). **Tasks:** `.5`, `.12`, `.16`.

The plan permits a new command signature and says `bead sync` settles the old receipts before
installation. Sync settles attempts, but intentionally leaves accepted `.command` receipts behind.
`Link.merge` deletes the pending JSON at `beadroll/sync.py:347`; `settle_commands` recognises a retry
by its receipt's exact signature at `beadroll/sync.py:519`. Empty `pending_ownership` does not imply
there are no receipts. An old request can consequently execute again after the prescribed upgrade.

**Reproduction:** In two scratch machines, A publishes `update X --claim --title from-A` and exits
through `BEAD_TEST_CRASH_AFTER_PUSH`. B then writes `from-B`. A runs `sync`. Both control and
experimental branches now report `pending_ownership=[]`, `integrity=[]`, one receipt, and `from-B`.
Retrying A's identical command preserves `from-B` in the control. Making the signature unrecognised
causes the retry to overwrite it with `from-A`, still exiting 0.

There is no Rust binary yet: the experimental branch changes only receipt whitespace to emulate
the explicitly proposed signature mismatch. It demonstrates the consequence through the existing
Python recovery implementation. This is the same lost-update invariant protected by G08 at
`tests/test_hardening.py:566`.

**Smallest correction:** Retain Python-compatible receipt signatures through the mixed fleet and
rollback period, or specify a migration that preserves request identity. Deleting receipts is not
such a migration. Add a same-store Python-to-Rust retry test after an accepted but unacknowledged
command, including an intervening edit by another machine. Apply upgrade checks to every workspace
on the machine, and prevent an older process from creating another incompatible receipt during
replacement.

### S02 — Medium: the proposed integer parser can write events Python drops

**Plan:** §2.3a (`docs/RUST-PORT.md:245`, `:270`) and §5 row 2. **Task:** `.2`.

Python integers have arbitrary precision, but their default JSON decimal conversion is bounded.
`loads_strict` delegates integer parsing to `json.loads` (`beadroll/base.py:60`), inheriting the
interpreter's digit limit. The proposed Rust parser keeps arbitrary digit strings without naming
that limit. A Rust-imported extra containing a sufficiently large integer could therefore make
the entire create event unreadable on Python, violating the mixed fleet contract.

**Reproduction:** With Python 3.14.7 and `sys.get_int_max_str_digits() == 4300`, synthetic create
events identical except for an integer in `data.extra` produced:

| Decimal digits | Events read | Issues replayed | Read problem |
|---|---:|---:|---|
| 4,300 | 1 | 1 | none |
| 4,301 | 0 | 0 | `line is not valid JSON` |

This is Python-side evidence of the actual boundary; no Rust parser was tested.
[Python's JSON documentation](https://docs.python.org/3/library/json.html#json.load) records this
default conversion limit beginning with Python 3.11.

**Smallest correction:** Specify the digit-limit policy, including the chosen oracle configuration,
and put 4,300/4,301-digit cases in parser, writer, import and replay differential fixtures. Preserve
the supported Python readers' accepted domain, or explicitly align those readers before enabling
larger Rust writes. This needs its own compatibility decision; neither approved rejection covers it.

### S03 — Medium: `process_group(0)` does not detach Git from the terminal

**Plan:** §2.3f (`docs/RUST-PORT.md:340`). **Tasks:** `.3`, `.6`.

The instruction “Each child gets its own session (`process_group(0)`)” is incorrect. That API calls
`setpgid`, while Python's `start_new_session=True` calls `setsid`
(`beadroll/base.py:96`). A new process group still belongs to the controlling terminal's session.
The existing `V8_SyncUnderAControllingTerminalCannotPrompt` test at `tests/test_hardening.py:4813`
depends on Git's transport being unable to open `/dev/tty`.

**Reproduction:** A Rust 1.99.0 runner using exactly `CommandExt::process_group(0)` and a Python
runner using `start_new_session=True` launched the same `/dev/tty` probe from a controlling PTY:

```text
rust_process_group: rc=0 TTY_OPEN
python_new_session: rc=0 NO_TTY
```

Thus a transport can prompt or wait for terminal input instead of failing promptly. The
[Rust CommandExt documentation](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#tymethod.process_group)
also distinguishes process groups from sessions; its `setsid` builder is still nightly-only on
the selected stable toolchain.

**Smallest correction:** Replace `process_group(0)` with a real `setsid` call in the audited
pre-exec runner path, alongside signal restoration. Do not first make the child a process-group
leader and then call `setsid`. Retain the controlling-terminal test and group-kill timeout tests
as runner acceptance checks.

### S04 — Medium: Studio needs a persisted source migration

**Plan:** §7 bridge (`docs/RUST-PORT.md:951`), especially the claim that nothing downstream of the
adapter changes. **Task:** `.15`.

Changing `CanonicalRef` and the reader is insufficient. Studio's database independently restricts
`canonical_ref` to `refs/heads/*` in
`migrations/20260909080058_project_identity.sql:117`. Its clone path also strips that prefix for
`clone --branch` in `crates/pancake-studio/src/sources/reader.rs:525`.

An implicit reader redirect has a second problem: refresh passes the last published code-branch
commit as the ancestry baseline (`crates/pancake-studio/src/sources/worker.rs:427`). Bead's journal
begins an independent history (`beadroll/sync.py:427`), so this transition cannot pass the existing
ancestry check merely because later journal commits fast-forward.

**Evidence:** Static inspection of the schema, clone path, refresh baseline and initial bead commit.
No Studio database migration or live refresh was executed. Studio already has the necessary source
revision mechanism: `crates/pancake-studio-db/src/sources.rs:168` increments generation on a
repository/ref change, and `:211` clears the ancestry baseline.

**Smallest correction:** Explicitly assign the database constraint migration, arbitrary-ref fetch
path, and transition through the existing source revision mechanism to `.15`. Keep the code branch
available for configuration discovery, and handle later configured tracker-source changes as new
revisions. Acceptance must migrate an existing publication and refresh it twice. Also qualify
`--depth=1`: it suffices for replay, while ancestry verification may still need the existing
deepening loop (`reader.rs:573`).

### S05 — Medium: Studio discovery omits the coordination endpoint

**Plan:** §7's source selection (`docs/RUST-PORT.md:954`). **Task:** `.15`.

The plan reads `.beadroll.json.ref` and fetches it from the code repository, but does not resolve
`.beadroll.json.remote`. A separate coordination repository is supported today
(`beadroll/machine.py:112`, `README.md:289`). Fetching the correct ref from the wrong repository can
find no tracker or an unrelated tracker.

**Evidence:** Static comparison of bead's remote precedence and config keys with the proposed
Studio discovery. Studio approves endpoints and selects credentials independently
(`crates/pancake-studio/src/sources/worker.rs:458` and `sources/policy.rs:417`), so following an
arbitrary tracked URL is not an adequate fix.

**Smallest correction:** Define the tracker binding as an approved endpoint, ref, expected workspace
and credential binding. Resolve tracked coordination remotes through Studio's existing source
approval mechanism; explicitly reject any unsupported remote-name or local-path forms. Require a
case where code and coordination live in different repositories, plus rejection of unapproved
redirection. This is separate from S04: a schema migration alone still selects the wrong repository.

### S06 — Medium: the infrastructure cutover gate still selects Python locally

**Plan:** Phase 7 (`docs/RUST-PORT.md:932`) and removal (`:1029`). **Task:** `.16`.

The four-machine gate requires Rust everywhere, but `infra/drive.py:35` hardcodes this checkout's
`bead` launcher for the local machine; `:45` uses it regardless of the installed binary. Before
removal the gate therefore runs Python locally. After removal it fails to start. The retained
`infra/roundtrip.py:4` and `infra/compat_real.py:10` likewise hardcode the deleted launcher.

**Evidence:** Static executable-path tracing. No real-machine drive or real-remote test was run.
The catalogue notices adaptation of `compat_real.py`, but the cutover task does not assign the
corresponding `drive.py` and `roundtrip.py` changes.

**Smallest correction:** Add explicit executable selection to all retained drivers before the
cutover gate, and have their preflight record/assert the version and selected path on each machine.
Make `.16` require a local Rust selection before deletion and a successful launch after deletion.
The Python language used by these drivers can remain unchanged.

## Other observations and checks

- All **458 tests** are represented exactly once in the three catalogues. Hardening uses
  class-level rows; checking method names alone would falsely report gaps.
- The epic's 16 children form an acyclic dependency graph. Only `.1` is currently ready. The
  implementation sequence and the parallel black-box test port have the expected prerequisites.
- Phase 5 may explicitly follow cutover. Studio's task has no blocking edge into `.16`; that does
  not prevent the epic from requiring both, and is not reported as a defect by itself.
- The failpoint grammar says every action defaults to the first hit (`docs/RUST-PORT.md:704`),
  while `backoff=skip` is intended to skip retry waits throughout a command (`:763`). Clarify repeat
  semantics or make this action repeat by definition. This is a smaller correction, not a separate
  material finding.
- Resolve `env!("CARGO_BIN_EXE_bead")` in the consuming integration test and pass the path into
  shared test support. Expanding it inside a separate support dependency failed a two-crate
  scratch build with exit 101; caller-side expansion compiled and passed. This follows
  [Cargo's environment-variable contract](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates).
  The plan does not require the wrong expansion site, so this is an implementation note, not a
  material finding. Preserve the environment and per-machine binary overrides either way.
- Cargo installation over a uv-style symlink was tested in a scratch root: `--force` replaced the
  symlink with the native binary and left its old target unchanged. No installation collision finding.
- The C-env catalogue notes already identify driver adaptations. The headline “unchanged” is too
  broad, but those notes prevent treating this as an unaddressed harness defect.
- Rust performance numbers in Appendix A are labelled estimates. They were not remeasured as
  implementation results, because the implementation does not exist yet.

## Validation, scope limits and side effects

The required `./check.sh` baseline was started before report creation and exited 0: all 455 tests
in the eight fast modules passed, in about 28 minutes. Its output was:

```text
test_hygiene   Ran 1 test in 0.067s OK (0s)
test_check     Ran 9 tests in 0.454s OK (0s)
test_replay    Ran 60 tests in 65.443s OK (66s)
test_storage   Ran 9 tests in 55.360s OK (55s)
test_compat    Ran 4 tests in 72.018s OK (73s)
test_commands  Ran 111 tests in 311.951s OK (312s)
test_protocol  Ran 54 tests in 295.971s OK (296s)
test_hardening Ran 207 tests in 866.175s OK (866s)
```

The ordinary compatibility module included its real-export/br/bv case. The separate `--real`
roundtrip option was not invoked. No implementation or existing test was changed by this audit.

Additional checks actually run:

- AST/catalogue comparison: 458 tests covered, no missing or duplicate entries.
- Read-only tracker inspection: the epic, all 16 children, readiness, and cycle checks.
- Synthetic receipt control and incompatible-signature scenarios, using local bare remotes.
- Synthetic integer-boundary parsing and replay, entirely in memory.
- Rust 1.99.0/Python controlling-terminal comparison.
- Two-crate Cargo failure and corrected-path control, offline.
- Scratch Cargo installation over a synthetic uv-style symlink, offline.
- Static inspection of Studio source persistence, approval, fetching and ancestry paths.
- Official Rust/Cargo/Python documentation checks for the external API claims above.

Not run: `--stress`, the full ownership sweep, `--real`, real-machine `drive.py`/`soak.py`, Studio's
database or test suites, native parity/mixed-fleet tests, or performance measurements. The broader
adversarial checks in `REVIEWING.md` §§5–6 were outside this plan audit; the ordinary baseline is
not claimed as new independent coverage of each checklist item. No new product regression test was
added for a not-yet-written Rust implementation; the plan findings use the focused proofs above.

Runnable scratch evidence:

- `/tmp/bead-rust-audit-compat.ZxhHle/receipts.py`
- `/tmp/bead-rust-audit-compat.ZxhHle/json_integer_limit.py`
- `/var/folders/hh/crd7tqlj5gn7n_gcvnnxc0040000gn/T/beadroll-port-audit-rdxlizcv/`
  (Rust terminal probe source/results and tracker snapshot)
- `/var/folders/hh/crd7tqlj5gn7n_gcvnnxc0040000gn/T/beadroll-port-cargo-audit-131z9t_2/reproduce.py`
- `/var/folders/hh/crd7tqlj5gn7n_gcvnnxc0040000gn/T/bead-cargo-install-audit-1jatlrt9/`

Side effects: scratch stores, repositories and compiled probes were created; receipt test
repositories were cleaned. The baseline creates its usual scratch fixtures. The real tracker was
used only for the repository-required audit claim and progress/result comments; these are work
tracking writes, not test traffic. No real test remote was used, no machine configuration changed,
and no external repository was edited. This report is the only repository file added. The epic is
left open for implementation and the audit claim is released after recording the result.

## Proposed FINDINGS.md rows

These are open plan findings, not closed implementation bugs. The existing ledger was not changed.

| Id | Severity | Finding | Status | Resolution | Tests |
|---|---|---|---|---|---|
| S01 | High | Sync-before-upgrade leaves receipts the proposed Rust signature cannot recognise | open | Preserve signatures or migrate request identity | Same-store retry across implementation upgrade, with intervening edit |
| S02 | Medium | Unlimited integer tokens exceed Python's JSON digit limit | open | Explicit compatible digit policy | Parser/import/replay boundary fixtures at 4,300/4,301 digits |
| S03 | Medium | Proposed process-group isolation retains the controlling terminal | open | Actual setsid in the runner | Port existing V8 controlling-terminal test |
| S04 | Medium | Studio bridge omits persisted source and ancestry transition | open | Schema, ref-fetch and source-revision migration | Existing publication migration plus two refreshes |
| S05 | Medium | Studio discovery omits separate coordination remotes | open | Approved complete tracker binding | Distinct code/coordinator and unapproved-redirection cases |
| S06 | Medium | Retained infrastructure drivers hardcode the removed Python launcher | open | Explicit binary selection and version preflight | Native selection before and after launcher removal |
