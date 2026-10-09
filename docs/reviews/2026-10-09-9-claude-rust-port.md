# Review 9: is the Rust port ready to start?

2026-10-09. Scope: the epic `roll-k2wy` and its 16 children, `docs/RUST-PORT.md`, the three
catalogues in `docs/rust-port/`, and what they say about the Python implementation, the test
suite, Rust 1.99.0, the sibling repositories they cite and pancake-studio. Plan commit `440fb8b`
(after review 8's corrections). Its Python baseline `9867715` is still the code on `main`: nothing
under `beadroll/`, `tests/`, `infra/`, `bead` or `check.sh` changed since. Studio was read at
`31cfafa`. macOS on arm64; Rust 1.99.0 (and the default 1.98.1); Python 3.14.7 and 3.11.15; git
2.55.0.

**Verdict.** With the corrections below applied, Phase 0 (`roll-k2wy.1`) can start, and so can the
test-port track after it. One decision is needed before Phase 6: §10's new decision 7, how studio's
builds reach a private `beadroll-core`. Six semantics rows are now decided in Phase 1 instead of
being discovered there.

The plan's factual base held up. Every on-disk format, git invocation, protocol constant, error
code, exit code and receipt rule it states matches the code, and every Rust feature and probe it
relies on works on 1.99.0. What did not hold were the edges:
- which commands write receipts, and where slice 2a ends;
- CPython semantics on replay and identity paths that §5 did not list;
- a lint configuration, a test layout and an install command that fail as written;
- harness gaps that would let the oracle count a Python run as a Rust pass;
- test bookkeeping that assigned tests to phases that cannot run them;
- studio's schema and build rules.

Every correction is applied in this change: to the plan, the catalogues, and the epic and 14 of its
children in the tracker. No product code changed.

## Findings

Ids T, ranked by what following the plan as written would do. "Applied" means the plan,
catalogues and issues now say the corrected thing; the work itself remains the children's.

### T01 — Medium: §5 misses CPython semantics that decide what replay reads and how identity is derived

**Plan:** §2.3a, §5, §4.2.3. **Task:** `.2`.

The plan's rule is that hostile input may differ only by a recorded decision. The differential
harness generates "hostile mutations of envelopes, payloads, duplicates, chains and ownership".
Six behaviours of the Python code fall outside both the ledger and those generators. In each, a
Rust port written from the plan would read a journal differently from a Python machine, or derive
a different workspace id:

1. **JSON read from bytes.** `loads_strict(line)` receives bytes. `json.loads` on bytes detects a
   UTF-8 BOM, UTF-16 and UTF-32 from the leading bytes, then decodes with `surrogatepass`. A
   journal line encoded as UTF-16-LE reads as a valid event today. A BOM is accepted on bytes and
   refused on text.
2. **Two lenient readers.** `complete_prefix` (the torn-tail repair, also applied to fetched
   segments) and `Link.fetch`'s manifest use plain `json.loads`. `NaN` parses there, and a final
   record holding it is kept, not cut. The manifest's `format` is compared with `!=`, so `true`
   and `1.0` pass as format 1.
3. **Unicode-aware `re`.**
   - On `str`, `\d` matches every decimal digit and `int()` converts it. So `free_key` renames a
     colliding `x.٣` to `x.3` inside replay, and `mint_key` and the merge's segment-name filter
     use `\d` too.
   - `(?i)[a-z]` also matches U+0130, U+0131, U+017F and U+212A. So
     `normalize_remote("httpſ://host/x")` is `host/x`, the same derived workspace id as
     `https://host/x`.
   - The plan lists `regex-lite` for the classifiers, and its `\d` and `(?i)` are ASCII.
   - `str.lower()` (row 9) also feeds `normalize_remote`, not only `search`.
4. **Replay's last-resort `except`** stores `repr(error)` as the integrity text. A fuzz over every
   operation's payload keys found it reachable three ways:
   - a truthy non-object `meta` on `dep_add`;
   - a list or object `key` beside `to` on `dep_remove`;
   - `free_key` on a numeric suffix over 4,300 digits, a `ValueError` from `int()`.

   The texts differ between Python versions: 3.14 says `cannot use 'dict' as a dict key
   (unhashable type: 'dict')`, earlier versions `unhashable type: 'dict'`.
5. **Uncaught exceptions.** An unparsable `BEAD_NET_TIMEOUT`, `BEAD_ROTATE_BYTES` or
   `BEAD_REPACK_*` raises at import, so even `bead --version` prints a traceback and exits 1. So do
   a non-numeric `*.jsonl` name in this machine's own journal folder (`int(last.stem)`),
   non-numeric `published` counts in `meta.json`, and `mint_key` over the digit limit.
6. **Nesting.** Row 7 said readable depth is "about 1k–10k levels" and kept a 10,000-level
   ceiling. Measured here, CPython 3.11.15 reads 994 levels, and 3.14.7, the reference interpreter,
   reads 116,199. Replay has no depth check, so a deep event applies on 3.14 and would be an
   unreadable line on Rust.

**Reproduction** (Python 3.14.7, a scratch `BEAD_HOME`, the package imported from the checkout):

```python
from beadroll import base, machine, model
base.loads_strict('{"a":1}'.encode("utf-16-le"))           # {'a': 1}
base.loads_strict(b'\xef\xbb\xbf{"a":1}')                   # {'a': 1}
machine.complete_prefix(b'{"a":1}\n{"a":NaN}')              # b'{"a":1}\n{"a":NaN}\n'
base.normalize_remote("http\u017f://host/x")                # 'host/x'
# a State whose key "x.\u0663" is taken: st.free_key("x.\u0663", uid) == 'x.3'
# replay of dep_add {"to": X, "type": "blocks", "meta": [1]} adds the integrity row
#   "event could not be applied: TypeError(\"'list' object is not a mapping\")"
json.loads("[" * 116199 + "]" * 116199)                     # parses on 3.14.7; 3.11.15 stops at 994
```

```sh
BEAD_NET_TIMEOUT=abc ./bead --version   # ValueError traceback from base.py:28
```

**Correction (applied):**
- §5 row 7 and new rows 22 to 26 state each behaviour, where it shows, and a recommended choice.
- Phase 1 decides them, applies every "reject" to Python as rows 3 and 6 were, and generates all
  six classes.
- Row 9 names identity.
- The reference interpreter is CPython 3.14.

### T02 — Medium: owner close and delete are described as coordinated commands with receipts

**Plan:** §6 slice 2c. **Task:** `.5`, which listed "coordinated: claim, release, takeover,
update --claim, and an owner's close and delete".

`coordinated` is called from exactly three places (`commands.py:267`, `:320`, `:421`):
- claim and takeover;
- release;
- `update` when it claims (`--claim` or `--status in_progress`).

`close` and `delete` go through `transition` (`commands.py:439`), a local write in `writing`. It
names the claim it ends, publishes through `publish_soon`, and answers `superseded` when a
takeover won. It writes no receipt and no pending attempt. `REVIEWING.md` §2 and `test_sweep`'s
`COORDINATED` set say the same. Following `.5` literally would give close and delete pending
attempts and receipts they do not have. That would change what a retry of either does after an
unanswered publish, which is what the sweep's owner close/delete schedules (G12) pin.

The description of the signature also omitted that `args` holds `command`, the subcommand's
name, and every destination of the subcommand with its default:

```text
claim p-1 -> {"actor": {...}, "args": {"command": "claim", "ids": ["p-1"], "offline": false, "remote": null}, "request": null}
update p-1 --claim --title T --estimate " +5" -> args holds all 22 destinations, "estimate": 5
```

**Correction (applied):**
- §2.3j and slice 2c name the coordinated commands exactly, and say that close and delete write
  no receipt.
- The signature's shape is spelled out with an example.
- `.5` says the same; its fixtures must show that close and delete write none.

### T03 — Medium: the oracle could count a Python run as a Rust pass, and Phase 0 forbade the fix

**Plan:** §4.2.1, Phase 0. **Tasks:** `.1` ("The Python package, its tests and ./check.sh are
untouched"), `.16`.

Three tests reach a Python bead by a path other than `BEAD_UNDER_TEST`:
- **The real-export test in `test_compat`** finds its helper at
  `Path(support.BEAD).resolve().parent / 'infra' / 'compat_real.py'`. Under
  `BEAD_UNDER_TEST=target/debug/bead` that path does not exist. With the path fixed, the helper
  still runs `./bead` itself (`compat_real.py:10`), as `roundtrip.py` does (`:4`) for
  `check.sh --real`.
- **`test_prime_and_onboard…`** (`test_commands.py:1274`) reads `AGENTS.md` beside the binary.
  It is in the oracle's runnable set.
- **`V8_Python312`** runs a copy of the launcher.

The plan assigned both drivers to Phase 7 and forbade Phase 0 from touching tests. So the oracle's
exit criterion would either fail on a harness defect or pass against Python.

The skip list was also defined as only "driver" and "in-process-only" tests. Recounted from the
test sources, the oracle can run 332 tests: 317 drive only the CLI, and 15 also inspect the store
in-process. The remaining 126 split into:
- 20 drivers;
- 75 in-process-only tests;
- 19 CLI tests whose verdict comes from patching in-process Python;
- 11 tests that run no `bead`;
- the real-export test, until its fix.

**Correction (applied):**
- Phase 0 makes the harness select the binary under test everywhere: the test modules,
  `compat_real.py` and `roundtrip.py` (`drive.py` stays in Phase 7). No Python behaviour changes.
- While the oracle targets another binary, Python bead refuses to start. The check sits in
  `cli.main`, so `V8_Python312`'s copy cannot bypass it either.
- §4.2 lists the four skip groups and the runnable count.
- `.1`'s acceptance allows exactly these Python-side changes, and `.16` keeps only `drive.py`.

### T04 — Medium: the test port assigned tests to phases that cannot run them

**Plan:** §4.1, §4.3, §6. **Tasks:** `.2`, `.5`, `.7`, `.8`, `.9`, `.10`, `.11`, `.12`.

All 458 tests are catalogued once, and the class totals hold, but what the counts mean did not:
- **B tests outside core.** 16 of the 76 B tests need `beadroll-git`, `-store` or `-sync`:
  `Workspace.ref` runs `git check-ref-format`, `normalize_remote` and `detect_actor` belong to
  other crates, and some tests write stores. Phase 1's exit, "all B-class tests are ported and
  green", could not be met by a core-only phase.
- **Tests that cannot pass against Python.** Five tests assigned to `.7` and `.8` need hooks
  Python lacks: two clock tests, kill-mid-append, kill-during-confirmation, and the
  clock-moves-back test. Yet those issues count a test only once it passes against Python. A
  sixth, retry exhaustion, passes against Python only with about 40 s of real backoff.
- **Double and missing owners.**
  - The 12 C-api tests were claimed by both `.5` and `.11`.
  - `test_replay`'s two CLI tests and `HelpAndPrime.test_every_command_has_help` belonged to no
    issue.
  - Neither were the in-process halves of f09 and of the `SyncRepoDamage` claim test, nor the CLI
    halves of the first `SyncRepoMaintenance` test and of G32.
- **"Crash hooks".** The 14 crash-hook tests in `test_hardening` are A†. 14 more in
  `test_commands` and `test_protocol` are labelled C-env, though Python already serves them. So 28
  tests rely on the hooks alone, and "C-env" mixed them with 6 tests that need new failpoints.
- **Wrong figures.**
  - §4.3 said `{g01…g64}`, but G65 exists.
  - It said "4 core" tests in `test_commands`, but none is core-only.
  - It put the `nearest_harness` test in `beadroll-sync`, but actor detection is the store's. The
    test also passes against Python as a black-box test with a fake `ps` on `PATH`, since Python
    runs `ps` from `PATH`.
  - §4.1's "about 1,600 call sites" is about 1,890, and `support.patched` is called 14 times, not
    11.
- **Naming.** The inventory's naming ("the same name … with the finding id") was not mechanical:
  finding ids are absent from most non-hardening names.
- **Stale porter notes in the catalogues.**
  - `LC_ALL=C` applies to `sync.git`'s git only; checkout-side git keeps the caller's locale,
    which G62 t3 relies on.
  - "Must not appear: mint a new / no answer" is pinned only by G53 and G18. The `IDENTITY` error
    does say "mint a new one".
  - A raw U+061C sat in the G60 row.
  - G30 t4's property was described as t1's.
  - The identity's temporary file is `.replica.PID.tmp`.
  - musl appeared in one matrix and not the other.

**Correction (applied):**
- §4.1 states what each count means, and §4.2 gives one mechanical naming rule: the module, the
  snake-cased class and the method, matched as ordered segments. It produces 458 unique paths.
- §4.3 and §4.6 are corrected, and Phase 1's exit covers the 60 core B tests. The other 16 land
  with Phase 2a, which lists them.
- The catalogue rows and porter notes are fixed.
- Each test-port child now names exactly which tests it ports:
  - `.7`: 104 tests;
  - `.8`: 38;
  - `.11`: 22 C tests plus the 6 that need new failpoints;
  - `.5`: the 11 C-api tests of `test_protocol`;
  - `.10`: `test_replay`'s two CLI tests and `xtask check`;
  - `.12`: the help test.

### T05 — Medium: slice 2a cannot port `sync` without pieces assigned to 2c

**Plan:** §6 slices 2a and 2c. **Tasks:** `.3`, `.5`.

2a ports `synchronise`, `Link` and `writing`, but 2c held "push outcomes", "fences, settling",
"maintenance, rebuild" and the receipt lifecycle. All of them are already in 2a's code paths:
- `synchronise` uses all four publish outcomes, `maintain` and `rebuild`. It sweeps non-attempt
  pending files with their receipts, fences when attempts are pending, drops them after a publish,
  and adopts a published tracker on `WORKSPACE_MISMATCH`.
- `Link.merge` confirms pending attempts found on the remote, and raises `ATTEMPT_FORK`.
- `writing` retires the actor's receipts through `forget_receipts`.

A 2a port that stubs these passes 2a's acceptance and diverges on the first store that holds a
pending attempt.

**Correction (applied):**
- 2a ports `Link` and `synchronise` whole, and `writing` with `forget_receipts`. 2c keeps
  `coordinated`, `settle_commands`, `settle_pending` and the claim verbs.
- `.3` gains a mixed acceptance check: a pending attempt and receipt left by Python are settled by
  Rust's sync as Python's would settle them, and the reverse.
- `.3` also starts the mixed-fleet CI job.

### T06 — Medium: the configuration in §3 fails CI on a skeleton, and §8's install commands fail

**Plan:** §2.2, §3, §8. **Tasks:** `.1`, `.16`.

On a skeleton of all seven members built with Rust 1.99.0 and the plan's `Cargo.toml`,
`clippy.toml` and `rust-toolchain.toml` verbatim:
- **Lints that fire.**
  - `println!` in `#[cfg(test)]` code fails `print_stdout`, because `allow-print-in-tests` is
    missing.
  - xtask cannot print at all.
  - `beadroll-test-support` is a library that the `allow-*-in-tests` keys do not reach, so its
    `expect`, `unwrap` and `panic!` fail.
  - `missing_docs` fires on the `bead` binary, xtask and test roots, and `unreachable_pub` on
    helpers in test modules.
- **Where exemptions can go.**
  - Members cannot add `[lints]` beside `workspace = true` ("cannot override
    `workspace.lints`"), so exemptions must be crate-level attributes.
  - A crate-level `#![expect]` fails once the lint stops firing.
- **A silent gap.** A misspelt lint in `[workspace.lints]` only warns, even under `-D warnings`
  ("the `unknown_lints` lint ignores `-D warnings`"). Denying `unknown_lints` and
  `renamed_and_removed_lints` in the table makes it fail.
- **Missing pieces.** `cargo xtask` needs an alias in `.cargo/config.toml`, and nextest exits 4
  when it finds no test.
- **Install commands.**
  - `cargo install --git git@github.com:bpcakes/beadroll.git …` fails before any network access:
    "invalid url … relative URL without a base; try using `ssh://…`".
  - A `--git` install never reads the repository's `rust-toolchain.toml`, so the 1.98.1 default
    fails `rust-version = "1.99"`.
  - `cargo install --path` reads the toolchain file from the current directory, not from the path.
- **Test-support builders.** Core's own unit tests cannot use them: the dev-dependency cycle
  builds a second `beadroll_core` (E0308).

The fixes:
- crate-level allows: xtask's prints, and test-support's unwrap, expect, panic and print;
- `//!` docs on every crate root, and `#![cfg(test)]` on test roots;
- `allow-print-in-tests`, and the two denied meta-lints.

With them, clippy, nextest, doctests and cargo-deny pass with no warning.

Everything else the plan says about Rust 1.99.0 held:
- every feature in "Recent Rust" compiles on stable;
- `CommandExt::setsid` is unstable (E0658), and `process_group(0)` followed by `setsid` fails
  with EPERM;
- under a PTY, a `setsid` child cannot open `/dev/tty` (ENXIO), while a `process_group(0)` child
  can;
- the `__mod_init_func` probe tells `0<&-` from `< /dev/null` in debug, release and LTO builds;
- `rustix::fs::fsync` is plain `fsync`: 0.032 ms, against 4.85 ms for `sync_all`;
- no resolved dependency declares an MSRV above 1.99.

**Correction (applied):**
- §2.2: the print rule, the exemptions and crate-root docs;
- §3: the two meta-lints, `allow-print-in-tests`, and a note of what was checked;
- §4.4: the builders serve core's integration tests;
- Phase 0: the lockfile, the alias and a first test;
- §8: both install commands.

`.1` is updated.

### T07 — Medium: studio's schema, policy and image build contradict §7

**Plan:** §7, §3 (studio's toolchain). **Task:** `.15`.

Read at studio `31cfafa`:
- **One source per project.** `studio_project_sources.project_id` is the primary key, and refresh
  cycles, publications and approval receipts each record one repository, ref and commit. "Keep the
  code branch as the configuration source" and "migrate `canonical_ref` to admit the coordination
  ref" cannot both hold in one row.
- **Approval is per host.** `authorize_remote` admits "any canonical repository path on an
  operator-enabled remote endpoint", and some credential scopes cover a whole host. So "pass
  coordination endpoints through studio's existing approval checks" would let `.beadroll.json`
  redirect within a host.
- **Other constraints:**
  - `studio_source_revision()` enforces generation and access revision as a trigger, not only in
    `sources.rs`;
  - the reader reads one ref and one blob per refresh;
  - publications admit only `input_profile = 'beads-rust-jsonl-v1'`;
  - studio's limits (depth 64, titles of 500 characters, 10,000 issues) are stricter than bead's;
  - `pancake-studio-core/AGENTS.md` says persisted refs are `refs/heads/...`.
- **The image build.** `infra/images/Dockerfile` says "no build argument, secret mount or layer
  carries a secret", and CI fetches git dependencies anonymously. The plan's deploy key for the
  image builder breaks that rule. The image builder also pins `rust:1.97.1` and refuses a
  mismatch.
- **Dependencies.** Studio's graph has `indexmap` 2, not `uuid`, which core does not use.
- **The tracker delta.** The 30-second delta exists only on the unmerged `feat/execution-runs`
  branch, where it diffs `.beads/issues.jsonl` files that bead never writes.

**Correction (applied):**
- §7 lists these facts and adjusts the bridge:
  - the tracker binding gets its own row or columns with its own access revision;
  - only an operator's source command sets it, and a mismatching `.beadroll.json` fails the
    refresh;
  - the trigger, the two tips, the limits case, the native mapping's second input profile, the
    image builder's toolchain and the execution-runs note.
- New open decision 7 (§10): how studio's builds reach a private `beadroll-core`. Recommended:
  vendoring it at a pinned revision.
- `.15` is updated to match.

### T08 — Low: the planned test layout does not compile

**Plan:** §4.3, §4.4.

`tests/cli.rs` declaring `mod commands;`, with its files under `tests/cli/`, fails with E0583:
"file not found for module `commands`". A crate root's modules resolve beside it, in `tests/`,
and a `tests/commands.rs` would become a second test binary.

`tests/cli/main.rs` with `tests/cli/commands.rs` and `tests/cli/commands/*.rs` compiles, passes
`-D clippy::mod_module_files`, and is one `cli` target.

**Correction (applied):** §4.3, the `test_replay` row, §4.4.

### T09 — Low: CI cannot be green in Phase 0 as specified, and two xtask commands had no owner

**Plan:** §4.9, §4.2.5, §1.4.
- In Phase 0 the oracle job "against the built binary" runs a skeleton and fails everything, and
  nothing said when it gates.
- Goldens of "every help page" from Python contradict §1.4, which makes help layout clap's.
- `xtask golden` and `xtask check` belonged to no phase.

**Correction (applied):**
- Each CI job joins with the phase that gives it something to run.
- The oracle runs against `./bead`, where it must pass completely, and against the binary, where
  it reports and gates from the close of 2d.
- Goldens hold each help page's usage line and options.
- `golden` belongs to 2b (`.4`), `check` to `.10`.

### T10 — Low: Phase 1 gates on a Phase 4 estimate

Phase 1's exit required replay of 10k events in 20 ms. Appendix A calls that figure an estimate
"for Phase 4 to confirm", and §4.7 says timings are evidence, not gates.

**Correction (applied):** Phase 1 measures and records it; Phase 4 owns the budget.

### T11 — Low: the cutover's real-store differential cannot be identical

`info` reports `version` (0.1.0 against 0.2.0), and `claims` computes `idle_minutes` from the
clock.

**Correction (applied):** the outputs must be equal apart from those two (§6 Phase 7, `.16`).

### T12 — Low: the environment and the process-tree walk were incomplete

Appendix C omitted:
- what the product reads implicitly: `HOME`, `TMPDIR`, `PATH`;
- the git variables `Link` extends or tests: `GIT_CONFIG_COUNT` with its `KEY_n` and `VALUE_n`,
  `GIT_SSH`, `GIT_SSH_COMMAND`, and the stripped location variables;
- the sweep's `BEAD_SWEEP_GC` and `BEAD_SWEEP_CONCURRENT`.

§2.2 called `detect_actor` pure, but with both a Claude and a Codex session variable set it runs
`ps` up to 12 times.

**Correction (applied):** Appendix C rows. §2.2 places the `ps` walk behind a function the CLI
passes in, and keeps it a `PATH` lookup, so a fake `ps` drives the test against both binaries.

### T13 — Low: several "house" attributions were wrong

Checked read-only in the sibling repositories (jig-sh `5bc4776`, hushgit `7423ea4`, runledger,
runlimit, batter, studio). These did not hold:
- hushgit has no "exactly one crate runs git" rule; its CLI runs git directly;
- jig's library is not "documented as existing for the binary and its tests";
- jig has no rule that "timing is evidence, not a test gate"; its CI uploads timings;
- proptest is used only in hushgit and runledger;
- no repository states the rule against a trait with one implementation; pancake-studio-core's
  `AGENTS.md` has the one against abstractions before their slice;
- jig's `nextest-version` is a minimum, its `slow-timeout` has no `terminate-after`, and it caps
  `test-threads` at 4 because git- and process-heavy tests fail spuriously at full core count;
- runledger, whose `deny.toml` the plan copies, is archived.

These held: the file budget, crate guides, clap 4.5, `File::lock` in hushgit, jig-state, the
runlimit rule, studio's `line-tables-only`, one test binary in pancake-studio-beads,
`#[ignore = "…"]`, and studio's toolchain and dependencies.

**Correction (applied):**
- the wording in §2.1, §2.2, §2.3i, §3, §4.7, §4.8 and §4.9;
- jig-sh's `rust-tests.yml` named as the CI template;
- the nextest thread count left to measurement.

## What was checked, and how

- **The Python package** (`beadroll/`, 3,784 lines) was read in full against §1, §2.3, §5 and the
  appendices. Its formats, separators, file and ref names, git commands, environment, retries,
  timeouts, push classification, repack thresholds, receipt lifecycle, the 27 commands and all 27
  error codes match the plan, except as T01, T02 and T12 say. The code has not changed since
  `9867715`.
- **CPython probes** on 3.14.7 and 3.11.15 covered:
  - each behaviour in T01;
  - a payload fuzz: 13 operations × up to 18 data keys × 13 value types through
    `payload_problem` and `replay`, collecting every last-resort message;
  - the receipt signature of each coordinated command;
  - malformed `BEAD_*` numbers;
  - a bisection of the readable JSON depth.
- **Rust 1.99.0**, with every cargo invocation in a scratch directory:
  - a seven-member skeleton with the plan's configuration verbatim;
  - fmt, clippy, nextest, doctests and cargo-deny 0.19.7, run against runledger's `deny.toml`
    with the advisory database cached and not fetched;
  - each listed language feature;
  - the closed-stdin probe and the controlling-terminal probe under a PTY;
  - the `cargo install` URL forms (against `example.invalid`) and toolchain selection;
  - the nextest configuration keys;
  - the integration-test layout.
- **The catalogues**:
  - all 458 test methods enumerated with `ast`, with no missing, extra or duplicate rows;
  - classes recounted per module;
  - each C, C-env and C-api test's seam checked against §4.5;
  - the oracle's runnable set and the launcher bypass paths derived from the test sources;
  - the naming rule applied to all 458 ids, giving unique paths;
  - about 20 porter-note claims checked against the code, of which five were stale (T04).
- **Sibling repositories and studio**: read-only. Their files, migrations, workflows and lockfiles
  were read; nothing was built, fetched or run in them.
- **The tracker**: the epic and all 16 children. The dependency graph is unchanged and acyclic, and
  `roll-k2wy.1` is the only ready child before and after.

**Not run:**
- `./check.sh --stress`, the full sweep, `--real`;
- `infra/drive.py` and `soak.py`;
- studio's tests and migrations;
- any real remote, network push or SSH connection.

There is no Rust implementation to test. Every Rust result above comes from scratch probes.

## Baseline

`./check.sh` at `440fb8b` with a scratch `BEAD_HOME`, started before the document changes were
complete. The edits touch only documents; `test_hygiene` was rerun on the final text.

```text
test_hygiene   Ran 1 test in 0.081s OK (0s)
test_check     Ran 9 tests in 2.306s OK (2s)
test_replay    Ran 60 tests in 63.669s OK (64s)
test_storage   Ran 9 tests in 15.999s OK (16s)
test_compat    Ran 4 tests in 43.520s OK (44s)
test_commands  Ran 111 tests in 401.475s OK (401s)
test_protocol  Ran 54 tests in 275.811s OK (276s)
test_hardening Ran 207 tests in 748.287s OK (749s)
```

All 455 fast tests passed, exit 0, in 1,553 s. `test_compat` ran its real-export, `br` and `bv`
case against the Python launcher, as it does today.

## Side effects

- **The tracker** (the real one, as `AGENTS.md` directs; every write is published to the
  coordination ref):
  - a claim on `roll-k2wy`, released at the end;
  - progress and result comments on it;
  - new descriptions and acceptance criteria for the epic and children 1 to 12, 15 and 16.

  The dependency graph and statuses are otherwise unchanged.
- **The repository**: this report, `docs/RUST-PORT.md`, the three catalogues, `docs/FINDINGS.md`
  and `docs/REVIEWING.md`. No product code, test or script changed.
- **Scratch.**
  - Python probes, a Rust scratch workspace and its builds, scratch stores and git repositories,
    all in the session's scratch directory.
  - cargo downloaded crates from crates.io into its registry cache.
  - The toolchain probe briefly wrote one file under `/tmp` and deleted it.
- **Not touched**: no global configuration (rustup default, git, SSH or shell), no installed
  binary, nothing in any sibling repository.

## Proposed ledger rows

Plan findings, recorded in `FINDINGS.md` as "plan corrected" in review 8's style: the document,
catalogues and issues now address each one, and the children remain open until their work and
tests pass.

| Id | Severity | Finding | Status | Resolution | Tests |
|---|---|---|---|---|---|
| T01 | Medium | §5 missed CPython semantics on replay and identity paths: bytes decoding, lenient readers, Unicode `re`, last-resort reprs, tracebacks, interpreter-dependent depth | plan corrected | §5 row 7 and rows 22 to 26, decided in Phase 1 with generators for each; `roll-k2wy.2` | Required: differential classes for each row; Python tests for every "reject" |
| T02 | Medium | Owner close/delete described as coordinated commands with receipts | plan corrected | §2.3j and slice 2c name the coordinated commands; `roll-k2wy.5` | Required: signature fixtures per coordinated command; close/delete write none |
| T03 | Medium | The oracle could count a Python run as a Rust pass, and Phase 0 forbade the harness fix | plan corrected | Harness selection in Phase 0; Python bead refuses to start under the oracle; skip groups and the 332 runnable tests in §4.2; `roll-k2wy.1`, `.16` | Required: the oracle passes against `./bead` and fails a test that bypasses the binary |
| T04 | Medium | Test-port assignments that phases could not meet (B tests outside core, C-env without Python hooks, double and missing owners, wrong figures) | plan corrected | §4.1, §4.3, §4.6, Phase 1's exit, the naming rule, catalogue fixes; `roll-k2wy.2`, `.5`, `.7` to `.12` | `xtask inventory` (Phase 0) |
| T05 | Medium | Slice 2a could not port sync without parts assigned to 2c | plan corrected | 2a ports `Link`, `synchronise` and `writing` whole; `roll-k2wy.3`, `.5` | Required: Python's pending attempt and receipt settled by Rust's sync, and the reverse |
| T06 | Medium | The plan's lint configuration fails a skeleton's CI; its install commands fail | plan corrected | §2.2, §3, §4.4, Phase 0, §8; `roll-k2wy.1` | Phase 0's CI on the skeleton |
| T07 | Medium | Studio's one source per project, host-level approval and no-secret image build contradict §7 | plan corrected; decision 7 open | §7 and §10's decision 7; `roll-k2wy.15` | Required: studio's own acceptance cases in `.15` |
| T08 | Low | The planned integration-test layout does not compile (E0583) | plan corrected | `tests/<name>/main.rs` roots (§4.3, §4.4) | Phase 0's skeleton |
| T09 | Low | CI could not be green in Phase 0; help goldens contradicted §1.4; `golden` and `check` had no owner | plan corrected | §4.2, §4.9; `roll-k2wy.4`, `.10` | — |
| T10 | Low | Phase 1 gated on a Phase 4 estimate | plan corrected | Measured and recorded in Phase 1; Phase 4 owns the budget | — |
| T11 | Low | The cutover differential could not be identical (`version`, `idle_minutes`) | plan corrected | §6 Phase 7; `roll-k2wy.16` | — |
| T12 | Low | Appendix C and the actor's `ps` walk were incomplete | plan corrected | Appendix C, §2.2 | — |
| T13 | Low | Several house attributions were wrong or overstated | plan corrected | §2.1–§4.9 wording | — |
