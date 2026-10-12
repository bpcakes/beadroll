# Review 11: roll-lqjj follow-ups

Independent Claude and Codex reports for `000dcec..08292dd`, preserved below.
Claude used the selected custom profile and completed on retry; its first invocation exited 1.
Both captures matched; no paths were excluded. The parent baseline passed: 783 tests
(2 leaky), 3 skipped, no doctest failures, and 14 infrastructure-driver tests.
The two retained Claude findings are B01 and B02 in `../FINDINGS.md`.

## Claude report (verbatim)

**Review of `roll-lqjj-follow-ups` against `main` (000dcec..08292dd)**

I found no substantive defects in the changed behaviour. There is one stale-documentation item and one test gap, both low severity. I did not run any tests: the review was read-only.

## Findings

- **[low] [crates/beadroll-sync/AGENTS.md:46] The crate's listed invariants still give the old retry constants**
  Root cause: roll-lqjj.7 changed `ROUNDS` to 24 and the wait to `U(0, min(0.1 × 2^(attempt−1), 2))` (`crates/beadroll-sync/src/synchronise.rs:20-37`). `docs/REVIEWING.md`, `docs/RUST-PORT.md` and the README were updated. The crate's own AGENTS.md still says "Exactly 12 rounds per command, `pause(U(0.1, 0.4) × attempt)`" under **Invariants**, and line 30 still says "the twelve rounds of a claim…". `docs/RUST-PORT.md:151` still opens with "exactly 12 rounds", though a later note corrects it.
  Impact: an agent following this crate's AGENTS.md would treat the new constants as a broken invariant and could "restore" 12 rounds. That would undo the maintainer's decision recorded in REVIEWING §2. The code is unaffected.
  Kind: supporting obligation.
  Recommendation: change both AGENTS.md lines to 24 rounds and the capped doubling wait, citing roll-lqjj.7.

- **[low] [crates/beadroll-sync/src/synchronise.rs:31-37] The new wait schedule has no test**
  Root cause: the only retry tests (`remote_ref.rs` exhaustion, g18) run with `backoff@always=skip`, which returns before the wait is computed. No test checks the doubling, the 2 s cap, or the `.min(8)` overflow guard.
  Impact: if the cap were dropped (waits of up to 25.6 s) or the exponent applied wrongly, every existing test would still pass. Only a `--contend` timing run would show it, and timings are not a gate. This touches R4 ("behavior changes covered by subject tests").
  Kind: supporting obligation.
  Recommendation: move the bound into a pure helper, e.g. `fn longest_pause(attempt) -> f64`. Test it at attempts 1, 2, 5, 6, 24 and `u32::MAX`, checking `0.1 · 2^(n−1)`, the 2.0 cap and no overflow.

## Requirement coverage

- **R1: satisfied.**
  - **Journal replay is unchanged.** The new issue field defaults to "in the issue" (`create.rs:103`), so replay behaves as before.
  - **Conflicts.** A `list` no longer reads texts. It still reports long fields set differently by two writers, from what each issue keeps of its texts (`query.rs:89-98`, `view.rs:37`). `show` reads those texts in, so its full conflict list (`view.rs:161`) is complete.
  - **Ownership.** `coordinated` and `writing` keep the same sequential/ownership split (`tx.rs:190-201`, `take_pending`). Nothing reads the transaction after its pending events are taken.
  - **Import.** The planner needs only keys and dangling dependencies, never texts (`import.rs:104-135`). Reading the prefix before the import is safe because an import never changes it.
  - **Where texts can change.** Only a set of a long field, a comment and a delete (via its extras, `claims.rs:109-117`) change texts, and all three must have them read in first (`extend.rs:147-166`).
  - **Commands on the outline view.** `list`, `ready`, `blocked`, `claims`, `dep`, `info`, `release`, `prime` and `sync` print briefs, titles and counts only. `dep list` uses only `detail()`'s dependency entries.
  - **Commands that read texts.** `show`, `comments`, `update`, `claim` and `takeover` read in their own issues' texts; `search`, `lint` and `export` still read every text.
- **R2: satisfied.**
  - **Checks on every read.** Texts are checked by place, length, 128-bit sum, file generation and the events behind each long field's register (`projection.rs:316-329`, `text_codec.rs:84-125`). Any mismatch falls back to replaying the journal: in `current`, `Tx::state` and `view_of`.
  - **Order of writes.** New texts are appended under the text file's lock, and the projection is written only after them (`store/projection.rs:449-473`).
  - **Concurrent restarts.** A competing restart of the text file can only produce a generation or sum mismatch, which leads to a replay; nothing wrong is ever read. Tests cover cuts, flipped bytes, a removed file, another issue's place, a SIGKILL between the two writes, and restarting a mostly unread file.
- **R3: satisfied for behaviour; the wait schedule is untested (finding 2).** Uncertain-push handling and receipts are unchanged (`ownership.rs:234-250`). The exhaustion test follows `ROUNDS`.
- **R4: mostly satisfied.**
  - `cargo xtask bead` counts heap in the same process with the same build identity throughout, so the import's runs stay consistent with each other.
  - The generated export, `--concurrent`, `--contend` and `--text` have parse and generator tests, and the plain history's digest is still pinned.
  - A failed bead command fails the whole measurement rather than being recorded as a result. That is consistent with the other modes.
  - The gap is the wait schedule (finding 2).

## Open questions

- `--contend` marks a write unpublished whenever the store has any unpublished events after it, not only that write's own. The docs read "unpublished" per write, so is this the intended measure?

## Test gaps

- The wait schedule (finding 2).
- No hostile test checks that a delete on an issue whose texts are not read in is declined. The randomised extension test probably covers it indirectly, since it compares texts read back after extending.

## Codex report (verbatim)

No actionable defects found in the pinned committed branch diff.

Both scope captures were complete and matched fingerprint `9b52749580d28a78d9d227c0786f6f9e7a02b9e2c4ae8b84fd7fdc8d07356819`. Reviewed all 66 changed files, including full diff evidence and relevant callers. No exclusions or working-tree changes.

| Requirement | Assessment and evidence |
|---|---|
| R1: Preserve replay, conflicts, ownership and import/export | Satisfied by inspected implementation and available coverage. Text loading precedes affected event application; unreadable or unavailable texts cause replay. Register ordering and causal tags survive encoding. Transaction events remain validated before append; consuming pending events preserves their order. Differential projection tests and CLI compatibility tests cover these paths. |
| R2: Disposable projection caches | Satisfied. [projection.rs](/Users/aa/Documents/beadroll/crates/beadroll-store/src/projection.rs:142) falls back to authoritative journals; [texts.rs](/Users/aa/Documents/beadroll/crates/beadroll-store/src/projection/texts.rs:37) checks generation and text blocks; [view_of](/Users/aa/Documents/beadroll/crates/beadroll/src/commands.rs:238) recovers failed selective loads through a full view. Added tests cover damaged, truncated, missing and foreign text data, compaction, and interruption after appending texts. |
| R3: Ownership and publication retries | Satisfied for reviewed changes. Retry tuning changes the round limit and delay without changing accepted, rejected or uncertain publication handling. [ownership.rs](/Users/aa/Documents/beadroll/crates/beadroll-sync/src/ownership.rs:226) consumes sequential events only after the command body finishes and retains ownership events for settlement. Exhaustion coverage verifies retained edits and absence of partial claims. |
| R4: Accurate timing and heap measurements | Implementation matches advertised workloads on inspection. Import heap measurement runs bead in a fresh counted process; subprocess failures propagate. Contention records unpublished writes and convergence; concurrent merge measurements create an independent local write before merging. Generator validity and deterministic output have tests. Published performance figures were not independently reproduced. |

Validation: the parent reported `./check.sh` exit 0: **783 tests passed, 3 skipped**, zero doctest failures, and **14 infra tests passed**. Complete baseline log: [baseline.txt](/var/folders/hh/crd7tqlj5gn7n_gcvnnxc0040000gn/T/beadroll-comprehensive-review-q6fe5t1n/baseline.txt). I did not duplicate that run.

Open questions: none requiring a change.

Test gaps and limitations: no demonstrated material coverage gap. This review did not run the slow stress models, full protocol sweep, real-remote experiments, or performance measurements. Concurrent cache replacement was assessed statically. No source, tracker, index, refs, or repository files were changed.
