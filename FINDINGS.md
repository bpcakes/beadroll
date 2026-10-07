# Findings ledger

Every finding from every review, in one place. The reports themselves are kept verbatim under
`reviews/`; this file is the living record of what each finding became. A finding is **fixed**
when the behaviour is changed and a test in `tests/` fails without the fix; **by design** when
the behaviour is kept on purpose (with the reason); **open** otherwise.

Reviewers: propose new rows in your report using the next free id for your review (see
`REVIEWING.md`); the maintainer merges them here.

## Review 4 (`reviews/2026-10-07-4-codex.md`, ids Q)

The report's findings section and its proposed ledger rows disagree on Q11 (offline close vs. `v: 0`);
both are recorded here, the second as Q12.

| Id | Severity | Finding | Status | Resolution | Tests |
|---|---|---|---|---|---|
| Q01 | High | Attempt files are called immutable but never compared; a damaged local copy is republished over the accepted one, and a malformed line still confirms the valid prefix | fixed | `Link.merge` compares attempt blobs: the accepted remote copy is authoritative, a differing local copy is kept as `*.fork-*` and reported; a pending twin that differs is `ATTEMPT_FORK`. `read_attempt` returns nothing for a file with any unreadable line | `test_protocol`: `test_q01_damaged_immutable_attempt_cannot_replace_remote_transaction`, `test_q01_malformed_attempt_is_inert_as_a_whole` |
| Q02 | High | Retrying an accepted state-dependent claiming update (parent move, close) rebuilt the command against mutated state: the new parent was removed, a second close appended | fixed | Recovery is per command, not per event: each pending attempt carries a receipt (`pending/<id>.command`: normalised args + actor); a retry whose receipt's attempt the remote accepted returns done without rebuilding. The event-level `already_done` from N01 is removed | `test_protocol`: `test_q02_retry_after_accepted_claiming_parent_move_keeps_parent`, `test_q02_retry_after_accepted_claiming_close_emits_one_close` |
| Q03 | Medium | A crash after local confirmation but before pending cleanup left a confirmed file and its pending twin; the retry fenced the twin and returned `NOT_OWNER` | fixed | Merge recognises the twin (equal bytes) as confirmed; the receipt then settles the retry | `test_protocol`: `test_q03_crash_after_confirmation_before_cleanup_is_idempotent` |
| Q04 | High | A future-format manifest was accepted, rewritten as format 1 and its unknown files dropped from the tip | fixed | `Link.fetch` refuses any manifest whose `format` is not this bp's (`FORMAT_UNSUPPORTED`) before merging or publishing | `test_protocol`: `test_q04_future_manifest_is_refused_without_remote_rewrite` |
| Q05 | High | `update --status tombstone` left a live claim; `release` then resurrected the issue | fixed | `update` refuses the tombstone spelling: `bp delete` is the one path, and it ends the claim | `test_commands`: `test_q05_tombstone_update_cannot_leave_a_releasable_claim` |
| Q06 | Medium | Importing a record before its dependency target, then the target, never produced the dependency; readiness differed from a one-shot import | fixed | A dependency on a key that does not exist yet is kept as such (`dangling` in the create payload) and becomes a real edge the moment the key exists, on every machine, tagged with the importing create so a later removal can take it away | `test_commands`: `test_q06_incremental_import_resolves_late_dependency_target` |
| Q07 | Medium | An invalid placeholder was exempt from the chain check yet advanced the version vector, so its tail appeared to have seen what it had not | fixed | Every event, placeholder or not, must have seen the last applicable event before it in its journal; one that has not is a chain break and lends no position. A quarantined id uses its own journal's copy for its deps | `test_replay`: `test_q07_invalid_placeholder_cannot_bridge_a_broken_machine_chain` |
| Q08 | Medium | `dep tree` recursed and overflowed on a 1,100-node chain | fixed | Explicit stack | `test_commands`: `test_q08_deep_valid_dependency_tree_does_not_overflow` |
| Q09 | Medium | Clearing `title`, `priority` or `issue_type` passed validation and crashed queries | fixed | `REQUIRED_FIELDS` cannot be cleared; two earlier register tests adapted to clear optional fields only | `test_replay`: `test_q09_required_register_clears_remain_queryable` |
| Q10 | Medium | A user field named `_dangling_dependencies` collided with private bookkeeping and crashed export | fixed | Dangling dependencies no longer live in `extra` (see Q06); unknown fields round-trip untouched | `test_commands`: `test_q10_reserved_dangling_extra_cannot_crash_export` |
| Q11 | Low | An offline close left provisional claims it had seen on the closed issue | fixed | A close removes every provisional claim in its causal past and keeps the concurrent ones | `test_commands`: `test_q11_offline_close_ends_a_provisional_claim` |
| Q12 | Low | An explicit `v: 0` executed as the current format | fixed | Any `v` other than this bp's is inert and reported | covered by `test_replay`: `test_unknown_event_version_is_not_applied_silently` (newer) and the format check |
| — | — | `find_cycles` verified against a brute-force oracle on all 66,066 digraphs of up to four nodes | verified | | `test_replay`: `test_d01_cycle_enumerator_matches_every_graph_through_four_nodes` |

## Review 3 (`reviews/2026-10-06-3-review.md`, ids N/P/C/K/D)

| Id | Severity | Finding | Status | Resolution | Tests |
|---|---|---|---|---|---|
| N01 | High | An accepted ownership attempt is rebuilt after a lost acknowledgement (release → `NOT_OWNER`, bundled edits applied twice) | fixed | `coordinated` keeps the result of an unanswered push and returns it when the next fetch finds the attempt on the remote; across processes, `Tx.already_done` recognises this actor's just-confirmed events (`release`, `close`, identical edits) | `test_protocol`: `test_release_recovers_a_successful_push_with_lost_acknowledgement`, `test_recovered_atomic_update_is_not_applied_again_after_a_later_observed_edit`, `test_timed_out_accepted_release_returns_success`, `test_crash_after_accepted_release_can_be_retried_idempotently` |
| N02 | High | A bundled attempt is confirmed file by file; readers and a crash see a claim without its edits | fixed | One file per attempt (`claims/<first-id>.json`, JSONL), written atomically; pending attempts likewise | `test_protocol`: `test_reader_sees_all_or_none_of_accepted_claim_and_bundled_edits`, `test_kill_during_local_confirmation_does_not_leave_partial_atomic_update` |
| P01 | High | Ancestry over ownership events stored as cumulative frozensets: n²/2 memberships, 2.4 GB at 10,000 events | fixed | One bitset (Python int) per event, equal bitsets shared; 61 MB at 10,000 events. A permanent implementation should use a reachability index | `test_replay`: `test_ownership_ancestry_stays_small_on_a_long_claim_release_history` |
| C01 | Medium | An identical event met first in another machine's journal reorders its real journal; `State.seq` read before the journal settled | fixed | `Journal.settle` orders by the owning record; `State` settles the journal first | `test_replay`: `test_identical_misplaced_duplicate_does_not_reorder_own_journal`, `test_identical_misplaced_duplicate_does_not_change_broken_chain_result` |
| C02 | Medium | Quarantined variants with scalar `deps`, or an agreed descendant dependency, crash replay | fixed | `effective_deps` accepts lists only; phase 1 skips causes that never applied | `test_replay`: `test_quarantine_malformed_scalar_deps_is_scoped`, `test_quarantine_agreed_cycle_is_scoped`, `test_quarantined_common_descendant_dependency_is_scoped` |
| C03 | Medium | A create without `issue` lands in `st.issues[None]`; an embedded comment missing fields crashes detail/export | fixed | A create must name itself; embedded comments need `text`, `author`, `created_at` (nullable) and a scalar `legacy_id` | `test_replay`: `test_create_without_issue_id_is_inert`, `test_missing_issue_creates_cannot_alias_and_overwrite`, `test_malformed_imported_comment_does_not_break_queries`, `test_malformed_embedded_create_comment_is_scoped` |
| C04 | Medium | A takeover supersedes a claim its writer had not seen | fixed | A takeover naming a claim it had not seen, or that is not a claim on the same issue, is inert and reported | `test_replay`: `test_takeover_must_causally_see_the_claim_it_supersedes` |
| K01 | Medium | Exported comment ids collide in a 7-digit range and are renumbered by arrival | fixed, caveat | 48-bit range from 2^47, assigned in id order (a function of the set, not of arrival); an imported id yields only if it collides with one of ours. A collision is now a one-in-10^14 event, which also means the tie-break is not exercised by any test | `test_replay`: `test_valid_colliding_comments_keep_exported_ids_when_other_machine_arrives`, `test_new_comment_integer_survives_collision_with_later_legacy_import` |
| D01 | Low | `find_cycles` misses elementary cycles its docstring promises | fixed | Johnson's algorithm, iterative, capped at 200 cycles | `test_replay`: `test_cycle_scan_reports_each_elementary_cycle` |

## Review 2 (`reviews/2026-10-06-2-reaudit.md`, ids R)

| Id | Severity | Finding | Status | Resolution | Tests |
|---|---|---|---|---|---|
| R01 | High | An uncertain push outcome led to rollback, then an own-journal fork | fixed | Claims left the journal: pending attempts, confirmation by presence on the remote, fence commits on the same head | `test_protocol`: `test_delayed_push_after_timeout_and_absent_fetch_must_not_fork`, `test_nonzero_applied_push_with_interleaved_writer_must_not_fork`, `test_fence_wins_against_a_real_push_still_running_after_timeout` |
| R02 | High | A second create replaced an existing issue | fixed | A create naming another id is inert (see also C03) | `test_replay`: `test_duplicate_create_uid_does_not_destroy_issue` |
| R03 | High | A failed "void" publication revived rejected claims and edits | fixed | Void records are gone; a refused attempt leaves nothing behind | `test_protocol`: `test_refused_void_publish_must_not_activate_unaccepted_claim`, `test_failed_void_publish_retry_must_not_activate_unaccepted_claim`, `test_failed_void_push_must_leave_original_claim_unconfirmed`, `test_failed_multievent_update_void_retry_must_not_commit_edits`, `test_rejected_fences_leave_attempt_pending_and_excluded` |
| R04 | High | An unlocked reader could confirm a rejected claim | fixed | Pending attempts are never read as events; readers see confirmed files only (see N02) | `test_storage`: `test_reader_never_confirms_a_claim_whose_push_was_rejected`, `test_refused_claiming_update_leaves_the_journal_untouched_under_a_reader` |
| R05 | Medium | Duplicate handling depended on arrival order | fixed | Raw records are settled together after loading (see also C01) | `test_replay`: `test_malformed_duplicate_order_does_not_change_projection` |
| R06 | Medium | An orphan takeover fenced another issue's close | fixed | Issue identity checked (see also C04) | `test_replay`: `test_orphan_takeover_does_not_fence_another_issues_close` |
| R07 | Medium | Quarantine could trap the whole causal tail | fixed | A quarantined id waits only for causes every variant agrees on; a stuck one is released inert | `test_replay`: `test_quarantined_variant_cannot_hold_descendants_in_cycle` |
| R08 | Medium | Payload validation permitted crashes and partial writes | fixed | Typed field validation, whole-event checks before any mutation, exception guard in replay | `test_replay`: `test_invalid_timestamp_is_scoped_even_when_event_is_inert`, `test_malformed_set_is_atomic_and_cannot_overwrite_internal_state`, `test_non_scalar_actor_thread_is_scoped`, `test_partially_invalid_label_add_applies_nothing` |
| R09 | Medium | A scoped null record disabled later sync | fixed | Records without a usable id are reported, not fatal, on the fast path too | `test_storage`: `test_invalid_own_remote_record_must_not_disable_sync` |
| R10 | Medium | First machine-identity write exposed an empty file | fixed | Written whole to a temporary file, linked into place | `test_storage`: `test_machine_identity_first_use_is_atomic_across_workspaces`, `test_machine_identity_can_recover_from_first_use_crash` |
| R11 | Medium | A long valid dependency chain overflowed the cycle scan | fixed | Iterative scan (now Johnson's, see D01) | `test_replay`: `test_deep_valid_dependency_graph_does_not_overflow_cycle_scan` |
| R12 | Low | A forced close through `update` did not publish like `close` | fixed | Same transition, same immediate publish | `test_protocol`: `test_forced_close_through_update_publishes_like_forced_close` |
| R13 | Low | A local import renumbered existing comments | fixed | Ids derived from the event (see K01) | `test_protocol`: `test_importing_legacy_comment_does_not_renumber_existing_local_comment` |

## Interim review (six findings reported inline, no file)

| Id | Finding | Status | Resolution | Tests |
|---|---|---|---|---|
| I01 | Marker written after the append it protected | fixed | Marker before append | `test_protocol` (lost-acknowledgement tests) |
| I02 | A sync that fetched before a lost acknowledgement voided the claim | fixed | Claims settle only against a head fetched under the lock | `test_protocol`: `test_a_sync_that_fetched_before_a_lost_acknowledgement_does_not_void_it` |
| I03 | `update --status closed` by the owner did not publish at once | fixed | `publish_soon` on any held claim ended by `update` | `test_commands` |
| I04 | Malformed events reached `apply_event` | fixed | Validation phase before apply | `test_replay`: `test_bad_event_schema_has_scoped_damage` |
| I05 | Ownership bookkeeping interleaved with application | fixed | Three-phase replay (classify, supersede, apply) | `test_replay` (permutation tests) |
| I06 | `--parent` could form a loop | fixed | Cycle check before emitting | `test_protocol`: `test_claim_update_cycle_validation_is_atomic_for_all_targets` |

## Review 1 (`reviews/2026-10-06-1-audit.md`, ids A)

| Id | Severity | Finding | Status | Resolution | Tests |
|---|---|---|---|---|---|
| A01 | High | Lost claim acknowledgement plus local work forked the journal | fixed | Superseded by the pending-attempt protocol (R01, N01) | `test_protocol`: `test_bug_local_write_after_lost_ack_must_not_fork_accepted_claim` |
| A02 | High | A reported foreign fork was then published | fixed | The published copy replaces the damaged one; ours is kept as `*.fork-*` | `test_protocol`: `test_bug_foreign_fork_report_must_not_replace_published_journal` |
| A03 | High | A torn tail poisoned the next write | fixed | `repair_tail` before any append or publish; fragment kept as `*.torn-*` | `test_storage`/`test_protocol`: `test_bug_torn_tail_followed_by_successful_append_must_stay_readable`, `test_bug_kill_mid_append_then_successful_write_must_stay_readable`, `test_bug_file_size_limit_partial_append_must_not_poison_next_write` |
| A04 | High | Segment boundaries hid a copied-machine fork; positions invented ancestry | fixed | Chain check per journal; an event not building on its predecessor is inert and reported | `test_replay`/`test_protocol`: `test_bug_copied_machine_id_fork_across_segments_is_detected`, `test_bug_segment_boundary_fork_must_not_erase_concurrent_title`, `test_non_chain_same_journal_must_not_invent_ancestry`, `test_non_chain_claim_cannot_bypass_pending_predecessor` |
| A05 | High | A quarantined takeover changed ownership by delivery order | fixed | Quarantined ids apply nothing | `test_replay`: `test_quarantined_takeover_cannot_supersede_a_valid_close` |
| A06 | Medium | An identical duplicate in another journal changed identity | fixed | Settled together; see C01 | `test_replay`: `test_identical_id_in_different_machine_journals_is_order_independent` |
| A07 | High | Repository-controlled workspace ids escaped `BP_HOME` | fixed | Opaque-id validation | `test_commands`: `test_workspace_config_cannot_escape_home` |
| A08 | Medium | Threads sharing a prefix shared owner identity | fixed | Owner is the full thread id | `test_protocol`: `test_distinct_threads_with_shared_first_eight_characters_are_not_same_owner` |
| A09 | Medium | Status updates bypassed claim guards | fixed | Same ownership rules for `update --status` | `test_protocol`: `test_nonowner_cannot_bypass_close_guard_with_status_update`, `test_claimed_issue_is_not_ready_after_nonowner_status_update` |
| A10 | Medium | A claiming update committed before validating its edits | fixed | One attempt; nothing is written if any part fails | `test_protocol`: `test_claiming_update_validates_parent_before_committing_claim` |
| A11 | Medium | The second parent's blocker was ignored | fixed | br's readiness rule over every parent | `test_replay`: `test_second_parent_blockers_hold_child` |
| A12 | Medium | Exported comment ids changed between exports | fixed | See R13/K01 | `test_compat`: `test_existing_comment_export_id_survives_clock_skewed_new_comment` |
| A13 | Medium | A conflicting event with a missing cause held descendants | fixed | See R07 | `test_replay`: `test_quarantined_event_with_missing_cause_does_not_hold_descendants` |
| A14 | Medium | Malformed events crashed replay; unknown versions ran silently | fixed | Validation; newer format and unknown ops are inert and reported | `test_replay`: `test_bad_event_schema_has_scoped_damage`, `test_unknown_event_version_is_not_applied_silently`, `test_unknown_operation_is_reported_and_does_not_mutate_timestamp` |
| A15 | Low | Comment id zero broke export | fixed | `is not None` checks | `test_compat`: `test_imported_comment_zero_id_roundtrips` |
| A16 | Low | Missing `ps` crashed harness detection | fixed | Tolerated | `test_commands`: `test_missing_ps_does_not_crash_nested_actor_detection` |
| A17 | Low | `BP_THREAD` alone did not override detection | fixed | | `test_commands`: `test_bp_thread_overrides_detected_thread_without_bp_agent` |
| A18 | Low | Global flags consumed operands after `--` | fixed | | `test_commands`: `test_global_options_respect_end_of_options` |
| A19 | Low | Parser errors bypassed the JSON error envelope | fixed | | `test_commands`: `test_parser_error_json_envelope` |

## Decisions reviewers questioned and that stand

Recorded so they are not re-reported; see `REVIEWING.md` §2 for the full list.

- Claims never expire and there is no heartbeat; a missing owner needs a human `takeover`.
- The display value for a conflicted field is the newest event; there is no resolution command.
- An owner's `close` is a local write published at once, not a conditional update.
- No persisted projection; every command replays the journal.
- One ref, serialised publishing; claims take seconds under heavy contention.
