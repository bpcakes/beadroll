<!-- Port catalogue for docs/RUST-PORT.md. Generated on 2026-10-09 by reading the Python tests at commit 9867715.
     Each row is a starting classification: confirm it when the test is ported, and correct it here if it is wrong. -->

# Test catalog: replay, storage, compat, check, hygiene, stress, sweep

Mechanism: `cli` = bead subprocess via `Machine.run/bead` (honours `BEAD_UNDER_TEST`); `inproc` = direct package calls;
`patched` = `support.patched`/`unittest.mock`; `driver` = Python subprocess doing `runpy.run_path(BEAD)` and monkeypatching
inside it; `env-fake` = fake binaries, env tricks, git hooks, chmod, SIGKILL, timeouts.
Port class: A = black-box CLI (Rust integration test, or this suite unmodified via `BEAD_UNDER_TEST`); B = pure in-process
logic, Rust unit test; C = needs a Rust injection seam (named); D = Python/harness-specific (Rust-side property named).

Totals: 86 methods. A 9, B 60, C 7, D 10.

## tests/test_replay.py (69): cli 2, inproc 66, inproc+env-fake 1. A 2, B 67 (the nine PortAlignment tests added in Phase 1)

Fixtures: `UnitTest.events` (uuid7() plus a real 2 ms sleep per event); `ReplayAuditFixture` (deterministic UUIDv7 ids
`((1_790_000_000_000+n)<<80)|(7<<76)|(2<<62)|n`, `snapshot()` = export_records + detail + ready + integrity + pending as
sorted JSON); `ReplayReaudit`/`ReplayThirdReview`/`ReplayResiduals`/`ReplayReview4` use fixed ids `019aXXXX-XXXX-7000-8000-<serial>`.
`ReplayReaudit` and `ReplayThirdReview` setUp patch `BEAD_HOME`/`BEAD_WORKSPACE` (incidental); `ReplayResiduals.setUpClass`
raises unless `BEAD_HOME` is set.

| class.method | subject | mechanism | seams/fakes needed | port |
|---|---|---|---|---|
| UnitTest.test_malformed_payloads_are_scoped_and_reported | Bad create/dep_add/actor events reported; valid events still apply | inproc | Journal.add, replay; uuid7() + 2 ms sleeps; integrity `problem[:6]` == "actor ", "create", "dep_ad" | B |
| UnitTest.test_an_ignored_newer_format_event_cannot_change_ownership | v2 takeover inert; owner's close stands; no stale entries | inproc | mutates `journal.events[id]` after add (build final event first in Rust); problem starts "event format" | B |
| UnitTest.test_one_remote_has_one_identity_however_it_is_spelled | Four URL spellings normalise to github.com/example/project | inproc | normalize_remote(str) -> str | B (beadroll-git's URL rules: lands with Phase 2a, not core) |
| UnitTest.test_the_coordination_ref_cannot_be_a_branch | heads/tags/bare/`..` refs rejected; refs/tracker/journal accepted | inproc | Workspace(wid,name,origin,config).ref raises BeadError; shells out to `git check-ref-format` | B (store and git: lands with Phase 2a, not core) |
| ReplayAuditProperties.test_six_machine_3000_event_replay_all_720_enumerations | 3000 random events: identical snapshot for all 720 delivery orders, duplicates | inproc | seed 6403, 30 issues, 6 replicas; each journal delivered forward+reversed; 16 random interleavings with random re-adds; heaviest replay test | B |
| ReplayAuditProperties.test_missing_causes_hold_claim_and_close_of_pending_create | Events with a missing cause wait; claim not applied | inproc | st.pending ids, issue claim None | B |
| ReplayAuditProperties.test_all_registers_keep_concurrent_clear_and_value_then_settle | Each register: concurrent clear vs value conflicts until merging write | inproc | REGISTERS, REQUIRED_FIELDS (clear rejected into integrity), conflicts(), issue.reg[field] values | B |
| ReplayAuditProperties.test_three_machine_add_wins_labels_and_dependencies | Labels/deps add-wins; removal removes only adds it saw | inproc | issue.labels set, issue.deps keyed (to, kind) | B |
| ReplayAuditProperties.test_cycle_after_merge_and_partial_removal | Merged dep_adds form cycle; removal unblocks one side | inproc | find_cycles, is_blocked | B |
| ReplayAuditProperties.test_tombstoned_blocker_unblocks_unless_status_disputed | Deleted blocker unblocks; concurrent reopen disputes status, re-blocks | inproc | conflicts()["status"], is_blocked | B |
| ReplayAuditProperties.test_handwritten_offline_takeover_does_not_fence_confirmed_owner | Offline takeover cannot fence online owner's close | inproc | claim, status, stale | B |
| ReplayAuditProperties.test_causal_write_wins_despite_backwards_clock | Causally later write wins despite older uuid7 timestamp | inproc | id rewritten to 01000000-0000-7000-8000-000000000001 | B |
| ReplayAuditProperties.test_reused_event_ids_are_isolated_across_workspaces | Same event ids in two workspaces replay independently | inproc+env-fake | patch.dict BEAD_HOME/BEAD_WORKSPACE; Workspace, append_events, load_journal on real files. Rust: pass home path explicitly (env is process-global under parallel tests) | B (store: lands with Phase 2a, not core) |
| ReplayAuditBugs.test_non_chain_same_journal_must_not_invent_ancestry | Same-journal position alone implies no causality | inproc | State.saw(a, b) | B |
| ReplayAuditBugs.test_non_chain_claim_cannot_bypass_pending_predecessor | Claim after pending predecessor in same journal not confirmed | inproc | chain rule | B |
| ReplayAuditBugs.test_identical_id_in_different_machine_journals_is_order_independent | Misplaced identical copy in other journal: same projection either order | inproc | Journal.add(event, physical replica) | B |
| ReplayAuditBugs.test_quarantined_takeover_cannot_supersede_a_valid_close | Two payloads under one id quarantined; close stands | inproc | exactly 1 integrity row; snapshot equality across orders | B |
| ReplayAuditBugs.test_quarantined_event_with_missing_cause_does_not_hold_descendants | Quarantined id with missing cause does not hold its child | inproc | pending == [] | B |
| ReplayAuditBugs.test_unknown_event_version_is_not_applied_silently | v999 event inert and reported | inproc | - | B |
| ReplayAuditBugs.test_unknown_operation_is_reported_and_does_not_mutate_timestamp | Unknown op reported; updated_at unchanged | inproc | - | B |
| ReplayAuditBugs.test_bad_event_schema_has_scoped_damage | Eight malformed envelopes never crash replay, all reported | inproc | json array event, missing id/op/replica, bad id, deps [1]/[{}], data array: Journal must take raw JSON values | B |
| ReplayAuditBugs.test_second_parent_blockers_hold_child | Blocker of a second parent holds the child | inproc | open_blockers -> keys == ["au-gate"] | B |
| ReplayAuditThreadOwnership.test_distinct_threads_with_shared_first_eight_characters_are_not_same_owner | Thread sharing 8-char prefix cannot release: NOT_OWNER | cli | Sandbox + 1 Machine; per-call BEAD_THREAD | A |
| ReplayReaudit.test_malformed_duplicate_order_does_not_change_projection | Corrupt copy elsewhere makes the id inert in either order | inproc | export_records equality | B |
| ReplayReaudit.test_quarantined_variant_cannot_hold_descendants_in_cycle | Quarantined variant naming its child as dep does not deadlock | inproc | causal order releases stuck quarantined ids | B |
| ReplayReaudit.test_invalid_timestamp_is_scoped_even_when_event_is_inert | Id with post-2100 timestamp reported even for unknown op | inproc | MAX_MILLIS check in envelope validation | B |
| ReplayReaudit.test_non_scalar_actor_thread_is_scoped | List-valued actor.thread makes event inert | inproc | - | B |
| ReplayReaudit.test_malformed_set_is_atomic_and_cannot_overwrite_internal_state | Set naming non-writable field "reg" applies nothing | inproc | field whitelist FIELD_TYPES; whole-event validation | B |
| ReplayReaudit.test_duplicate_create_uid_does_not_destroy_issue | Second create reusing an issue uid rejected | inproc | State.resolve(key) | B |
| ReplayReaudit.test_merged_parent_cycle_matches_br_readiness | Merged parent cycle: one cycle, both issues ready (br 0.5.7) | cli | 2 Machines, 4 sync rounds; `dep cycles` count, `ready` ids | A |
| ReplayReaudit.test_offline_forged_takeover_stays_provisional_and_does_not_fence_close | Offline takeover with generation 999 stays provisional; generation 1 | inproc | claim_generation | B |
| ReplayReaudit.test_orphan_takeover_does_not_fence_another_issues_close | Takeover naming another issue's claim is inert | inproc | - | B |
| ReplayReaudit.test_partially_invalid_label_add_applies_nothing | Label list with a non-string applies nothing | inproc | labels == set() | B |
| ReplayReaudit.test_six_machine_3600_mixed_envelope_events_are_scoped_and_deterministic | 3600 events, 3000 malformed: exact counts, order-independent | inproc | seed 901834; 18 sampled permutations; integrity len 3000, chain_breaks {} | B |
| ReplayReaudit.test_deep_valid_dependency_graph_does_not_overflow_cycle_scan | 1500-deep blocks chain: no recursion overflow, no cycles | inproc | iterative SCC/Johnson | B |
| ReplayReaudit.test_valid_six_machine_6000_events_converge_and_ancestry_is_exact | saw() equals exact ancestry; 36 delivery orders converge | inproc | seed 901833; 10,000 saw() checks vs bitset oracle; journal.order; double adds | B |
| ReplayReaudit.test_all_registers_preserve_none_against_concurrent_value | Concurrent clear-all vs set-all conflicts on every clearable register | inproc | FIELD_TYPES, REQUIRED_FIELDS | B |
| ReplayReaudit.test_missing_predecessor_holds_claim_and_close_then_recovers | Claim and close wait for missing create, then apply | inproc | incremental Journal.add then replay again | B |
| ReplayReaudit.test_ignored_op_with_valid_id_does_not_hold_descendant | Unknown op inert but still a valid cause | inproc | - | B |
| ReplayThirdReview.test_quarantine_agreed_cycle_is_scoped | Quarantined id in a cycle with its child: child survives | inproc | Journal.add(sequenced=...), settle() | B |
| ReplayThirdReview.test_quarantine_malformed_scalar_deps_is_scoped | Duplicate with scalar deps quarantines without crashing | inproc | - | B |
| ReplayThirdReview.test_malformed_imported_comment_does_not_break_queries | Embedded comment lacking author/created_at makes create inert | inproc | export_records, detail must not raise | B |
| ReplayThirdReview.test_create_without_issue_id_is_inert | Create lacking "issue" rejected; no None-keyed issue | inproc | - | B |
| ReplayThirdReview.test_new_comment_integer_survives_collision_with_later_legacy_import | Imported legacy comment id colliding with derived id is bumped | inproc | comment ids: 2^47 + sha256(event id)[:6] mod 2^47; legacy id +1 on collision | B |
| ReplayThirdReview.test_identical_misplaced_duplicate_does_not_reorder_own_journal | Misplaced duplicate never reorders owner journal | inproc | chain_breaks == {} | B |
| ReplayThirdReview.test_valid_colliding_comments_keep_exported_ids_when_other_machine_arrives | Late comment never renumbers an already exported comment id | inproc | hunts a collision under a retired 7-digit formula (up to 100k sha256); Rust: find a real 48-bit collision or make the id hash injectable | B |
| ReplayThirdReview.test_optional_register_clears_remain_queryable | Clearing description/design/notes/assignee keeps queries working | inproc | export_records, detail | B |
| ReplayThirdReview.test_merged_dependency_cycle_reports_remaining_cycle_after_partial_removal | Removing one edge leaves exactly [ex-one, ex-two, ex-three] | inproc | find_cycles exact output, is_blocked | B |
| ReplayThirdReview.test_mixed_detached_ownership_ancestry_matches_exact_graph | Unsequenced (claim-file) events: exact saw(); shuffles agree | inproc | seed 71029; every 3rd event sequenced=False; 10,000 saw() checks; 12 shuffles | B |
| ReplayResiduals.test_missing_issue_creates_cannot_alias_and_overwrite | Creates without "issue" never share a None uid | inproc | - | B |
| ReplayResiduals.test_identical_misplaced_duplicate_does_not_change_broken_chain_result | Broken-chain outcome independent of duplicate position | inproc | conflicts, integrity, chain_breaks | B |
| ReplayResiduals.test_takeover_must_causally_see_the_claim_it_supersedes | Takeover not causally after its claim is inert | inproc | State.saw | B |
| ReplayResiduals.test_quarantined_common_descendant_dependency_is_scoped | Quarantined id depending on its descendant: one integrity row | inproc | - | B |
| ReplayResiduals.test_malformed_embedded_create_comment_is_scoped | Bad embedded comment never breaks global export | inproc | - | B |
| ReplayResiduals.test_cycle_scan_reports_each_elementary_cycle | Elementary cycles (a,b,c) and (a,c) both reported | inproc | find_cycles | B |
| ReplayScale.test_ownership_ancestry_stays_small_on_a_long_claim_release_history | 3000 claim/release events replay under 1 s | inproc | wall-clock < 1.0 s; asserts private `st._shared` size (Python detail: keep time/memory bound only) | B |
| ReplayReview4.test_q07_invalid_placeholder_cannot_bridge_a_broken_machine_chain | Malformed placeholder lends no ancestry; chain break reported | inproc | actor [] ; chain_breaks contains "a" | B |
| ReplayReview4.test_q09_required_register_clears_remain_queryable | Clearing title/priority/issue_type rejected; queries still work | inproc | brief, line (human text), export_records | B |
| ReplayReview4.test_d01_cycle_enumerator_matches_every_graph_through_four_nodes | find_cycles equals brute force on all 66,066 graphs up to 4 nodes | inproc | duck-typed fake State (issues/waits_for/key): find_cycles must be generic over a graph trait | B |
| Review5Replay.test_e10_extra_cannot_override_canonical_export_fields | create.extra cannot override exported id/status/comments | inproc | payload_problem, export_records | B |
| PortAlignment.test_u01_numbers_beyond_a_float_are_not_json_numbers | 1e400 is not a JSON number, like NaN; such a line is unreadable | inproc | loads_strict, read_segment | B |
| PortAlignment.test_u02_ids_and_deps_with_a_trailing_newline_are_not_uuids | Ids and deps match the UUID shape whole | inproc | event_problem, Journal.add, replay | B |
| PortAlignment.test_u03_nesting_deeper_than_512_levels_is_unreadable_on_every_python | 512 levels read on every Python, no deeper; brackets in strings count for nothing | inproc | MAX_DEPTH, nesting, loads_strict, read_segment | B |
| PortAlignment.test_u04_journal_lines_are_utf8_whatever_their_first_bytes | BOM, UTF-16, UTF-32 and Latin-1 lines are unreadable; a lone surrogate reads as U+FFFD, reported | inproc | read_segment | B |
| PortAlignment.test_u06_key_numbers_are_ascii_digits | A renamed copy of `x.٣` keeps its name; mint_key counts ASCII children only | inproc | Journal.add, replay, renames, mint_key | B |
| PortAlignment.test_u07_payloads_that_reached_the_last_resort_are_refused_by_name | dep_add meta, dep_remove key and a 4,300-digit key number are refused by name | inproc | payload_problem, replay, integrity | B |
| PortAlignment.test_u08_imported_comments_carry_neither_a_uid_nor_a_where | An imported comment with a uid or a where is refused; good ones show and export | inproc | payload_problem, replay, detail, export_records | B |
| PortAlignment.test_u09_a_create_cannot_clear_created_at | A create clearing created_at is refused; other bookkeeping may be null | inproc | payload_problem | B |
| PortAlignment.test_u10_a_quarantined_id_names_its_issue_only_by_a_string | A quarantined stand-in names its issue only by a string | inproc | Journal.add, replay, integrity | B |

## tests/test_storage.py (9): cli 4, cli+env-fake 1, cli+driver(+env-fake) 2, driver+env-fake 2. A 5, C 4

All in `StorageReaudit`; setUp: Sandbox, Machine `storage-a`, `init --prefix ex`, one issue. `two()` adds `storage-b` after syncs.

| class.method | subject | mechanism | seams/fakes needed | port |
|---|---|---|---|---|
| StorageReaudit.test_invalid_own_remote_record_must_not_disable_sync | `null` line in own segment quarantined; sync keeps working | cli | appends to `$BEAD_HOME/<ws>/events/<own>/NNNNNN.jsonl`; info.integrity list len 1; sync.integrity == 1; second sync rc 0 | A |
| StorageReaudit.test_refused_claiming_update_leaves_the_journal_untouched_under_a_reader | Refused claiming update: one pending attempt, journal bytes unchanged, consistent reader | cli+driver+env-fake | driver pauses first `Link.publish` until go-file; reader driver pauses after `segment_files()`; BEAD_ROTATE_BYTES=1; Popen new session, SIGKILL cleanup; pending/*.json ops == [claim, set, label_add]; ALREADY_CLAIMED rc 1 | C: failpoints `publish.before` and `journal.after-enumerate`, action pause-until-file |
| StorageReaudit.test_reader_never_confirms_a_claim_whose_push_was_rejected | Reader racing a rejected claim never shows it confirmed | cli+driver | pauses first `Link.publish`; pauses `view()` before `load_pending` | C: failpoints `publish.before`, `view.before-load-pending` |
| StorageReaudit.test_machine_identity_first_use_is_atomic_across_workspaces | Two workspaces minting replica.json concurrently share one identity | driver+env-fake | wraps `os.link` to replica.json (pause); BEAD_WORKSPACE per process; 2 s timeout tolerated; second process is `python3 $BEAD` (breaks with a native binary) | C: failpoint `identity.before-link` (pause) |
| StorageReaudit.test_machine_identity_can_recover_from_first_use_crash | SIGKILL before linking identity: no half file; retry works | driver+env-fake | os.link wrapper sleeps 30 s, test killpg SIGKILL; expects no replica.json and a `.replica.*.tmp` in BEAD_HOME | C: failpoint `identity.before-link` + external SIGKILL |
| StorageReaudit.test_repair_preserves_a_complete_record_that_only_lacks_newline | Final record without newline kept; no torn backup | cli | strips trailing \n from own segment; no `*.torn-*`; list total 2; info.integrity [] | A |
| StorageReaudit.test_repair_foreign_fragment_before_publish_retains_authoritative_events | Torn tail in foreign copy repaired from remote; backup kept | cli | appends `{"v":1,"id":"unfinished` to b's copy of a's segment; exactly one `*.torn-*`; sync.integrity 0 | A |
| StorageReaudit.test_rewritten_foreign_file_preserves_local_backup_and_published_bytes | Rewritten foreign segment restored; damaged bytes kept as `.fork-*` | cli | sync.forked_journals == ["events/<replica>/NNNNNN.jsonl"]; one `*.fork-*` with damaged bytes | A |
| StorageReaudit.test_copied_identity_divergence_across_rotation_refuses_remote_change | Copied home (twin identity) diverging: JOURNAL_FORK, remote untouched | cli+env-fake | shutil.copytree of BEAD_HOME; BEAD_ROTATE_BYTES 1 vs 9999999; `git --git-dir origin rev-parse refs/beads/journal` unchanged | A |

## tests/test_compat.py (4): inproc 1, inproc+env-fake 1, cli 1, env-fake 1. A 2, B 2

| class.method | subject | mechanism | seams/fakes needed | port |
|---|---|---|---|---|
| CompatAuditCommentIds.test_existing_comment_export_id_survives_clock_skewed_new_comment | Exported comment id stable after clock-skewed later comment | inproc | hand-built UUIDv7 with ms 1000/3000/2000; Journal, replay, export_records; actor without thread | B |
| CompatAuditRealCompatibility.test_all_available_real_exports_and_br_bv_consumers | Real Beads exports round-trip; br readiness parity; bv parses export | env-fake | subprocess `infra/compat_real.py --out tmp` (180 s); needs `br` and `bv` (PATH or ~/.local/bin) and `~/Documents/*/.beads/issues.jsonl`, else skip. Helper path = dirname(BEAD)/infra and helper runs the checkout's `./bead`, ignoring BEAD_UNDER_TEST | A (resolve helper from source tree independently of selected binary; pass BEAD_UNDER_TEST through; optional external tools/data) |
| CompatAuditRefValidation.test_ref_names_reject_code_refs_and_traversal_accept_unicode | Six bad refs fail with CONFIG; unicode and custom refs accepted | inproc+env-fake | patch.dict BEAD_HOME/BEAD_WORKSPACE; Workspace().ref; `git check-ref-format` | B (store and git: lands with Phase 2a, not core) |
| CompatCustomStatuses.test_imported_custom_statuses_are_kept_and_never_ready | Imported custom statuses kept, never ready; bead refuses setting them | cli | `init --import`; export, ready, blocked, list --limit 0, show; human show has "by unknown"; VALIDATION rc 1; stderr has "unknown status" | A |

## tests/test_check.py (9): env-fake 3, patched 5, inproc 1. D 9

| class.method | subject | mechanism | seams/fakes needed | port |
|---|---|---|---|---|
| CheckScript.test_success_keeps_the_roundtrip_summary | `check.sh --real` success shows only roundtrip summary line | env-fake | copies check.sh to temp; fake `python3` sh script on PATH; CHECK_TEST_MODULE_STATUS / CHECK_TEST_REAL_STATUS; 10 s timeout | D: keep while check.sh drives; else test the new runner's exit status and summary |
| CheckScript.test_roundtrip_failure_fails_the_check_and_keeps_diagnostics | Roundtrip failure fails check.sh, keeps stdout and stderr | env-fake | same | D |
| CheckScript.test_successful_roundtrip_does_not_hide_a_failed_module | Failed module fails check.sh despite roundtrip success | env-fake | same | D |
| SweepFailureReporting.test_every_failed_schedule_fails_the_sweep | Each of 11 injected failing schedules fails Sweep, both modes | patched | mock.patch.object(test_sweep, schedules / SLICE / run_schedule); BEAD_SWEEP ""/full, GC=0, CONCURRENT=0 | D: meta-test of the Rust sweep runner |
| SweepFailureReporting.test_successful_schedules_pass | Passing schedule report lets Sweep pass, both modes | patched | same | D |
| SweepFailureReporting.test_default_slice_retries_crashed_owner_close_and_delete | Default slice includes close/delete crash-before-push retry-first pairs | inproc | reads test_sweep.SLICE | D: assert the Rust slice keeps these |
| RoundtripFailureReporting.test_identical_records_succeed | roundtrip.py: identical records exit 0, "TOTAL 1/1", init --offline | patched | runpy infra/roundtrip.py; mock glob.glob and subprocess.run; RT_HOME | D |
| RoundtripFailureReporting.test_failed_commands_fail_the_roundtrip | init/export/ready failure: non-zero exit, error text printed | patched | same | D |
| RoundtripFailureReporting.test_changed_records_fail_the_roundtrip | Changed export: non-zero, "TOTAL 0/1", DIFF line | patched | same | D |

## tests/test_hygiene.py (1): inproc 1. D 1

| class.method | subject | mechanism | seams/fakes needed | port |
|---|---|---|---|---|
| RepositoryHygiene.test_tracked_files_name_no_machine_or_user | No tracked file names a home dir, .local host, ssh target | inproc | `git ls-files -z` + three regexes; skips .png/.jsonl/undecodable; four placeholders allowed | D: language-agnostic repo lint, keep as is on the Rust repo |

## tests/test_stress.py (2): cli+inproc 2. C 2

| class.method | subject | mechanism | seams/fakes needed | port |
|---|---|---|---|---|
| RootAuditStress.test_six_machine_2400_operation_reference_model | 2400 seeded random ops, 6 machines, vs ownership/readiness model | cli+inproc | seed 20261006; soft partitions; 8 full checks; in-process load_journal+replay under swapped os.environ BEAD_HOME for journal event ids and claim_history | C: library API (open store at home+wid, load_journal/replay exposing claim_history) or a test-only state dump command |
| ConcurrentReaudit.test_six_machine_3000_concurrent_commands_with_partitions | 10 waves x 6 threads x 50 concurrent ops; ownership and blocker oracles | cli+inproc | seed 20261007; ThreadPoolExecutor(6) = 6 concurrent bead processes; in-process view(Workspace) under patch.dict BEAD_HOME; reads unconfirmed.json | C: same seam |

## tests/test_sweep.py (1): cli+driver+env-fake 1. C 1

| class.method | subject | mechanism | seams/fakes needed | port |
|---|---|---|---|---|
| Sweep.test_sweep | Ownership protocol: 9 ops x 7 fault boundaries x 12 follow-up pairs | cli+driver+env-fake | product hooks BEAD_TEST_CRASH_BEFORE_PUSH / AFTER_PUSH (exit 98/99); runpy WRAPPER replacing run_git (first push), confirm_ownership, write_atomic (torn write + SIGKILL); pre-receive stall hooks + BEAD_NET_TIMEOUT=1.0; optional post-receive `git gc`; pgrep/ps checks | C: failpoints keyed by boundary name, set by env, present in every build and inert without configuration |

## Appendix: sweep fault boundaries

| boundary | coordinated ops (claim, release, takeover, update-edits/-parent/-closed) | close, delete, sync |
|---|---|---|
| crash-before-push | `BEAD_TEST_CRASH_BEFORE_PUSH=1`: product `os._exit(98)` after the pending receipt (`pending/<id>.command`) and attempt (`pending/<id>.json`) are written, before `Link.publish` | wrapper `push-before`: `run_git` patched, first `git push` -> `os._exit(98)` before pushing |
| crash-after-push | `BEAD_TEST_CRASH_AFTER_PUSH=1`: `os._exit(99)` after publish returned "ok", before confirm/drop (lost ack) | wrapper `push-after`: real first push; if rc 0 -> `os._exit(99)` |
| timeout-fenced | origin `hooks/pre-receive` STALL_FENCED: first push (mkdir lock) sleeps 3.5 s then exits 0; later pushes pass at once, so the fence lands first. Client `BEAD_NET_TIMEOUT=1.0` | same |
| timeout-landed | STALL_LANDED: first push sleeps 3.5 s and lands; later pushes poll 50 ms (max 400) for hook-finished, sleep 0.3 s; the fence's ref update fails | same |
| kill-confirm | wrapper: call real `confirm_ownership(ws, events)`, then SIGKILL self if events (confirmed in claims/, not dropped from pending/) | n/a |
| kill-write-pending | wrapper `write_atomic`: target in `pending/` with `.json` suffix -> write first half of bytes to `.<name>.<pid>.tmp`, fsync, SIGKILL self | n/a |
| kill-write-confirm | same for target in `claims/` | sync only (copying another machine's attempt into claims/) |

Wrapper invocation: `[python3, fault.py, $BEAD, *argv, --json]` with `BEAD_SWEEP_FAULT=<name>` (popped from env); it runs
`runpy.run_path(BEAD)`, wraps every `beadroll*` module's globals in an `Everywhere` dict (read from first holder, write to all),
patches, then `SystemExit(main(argv))`. Each fault fires once per process (`fired` list for pushes).
Judge excuses rc 98, 99, -9 and codes UNRESOLVED / REMOTE_UNREACHABLE / SYNC_CONFLICT: a Rust kill must be a real SIGKILL
(-9), not abort/panic. After the op the harness waits up to 20 s for hook-finished and for no `receive-pack.*<origin>`
process (pgrep), then deletes the hook.

Proposed Rust failpoint names: `push.before`, `push.after-ok` (git transport layer, first hit by default), `ownership.before-publish`
and `ownership.after-publish-ok` (aliases of the two BEAD_TEST_CRASH_* env hooks, which test_commands, test_hardening and
infra/drive.py S6 also use against normal builds), `ownership.after-confirm`, `atomic-write:pending/`, `atomic-write:claims/`
(torn write then SIGKILL), plus storage's `publish.before`, `journal.after-enumerate`, `view.before-load-pending`,
`identity.before-link` (pause: touch ready-file, poll go-file). Actions: exit(N), sigkill, torn-sigkill, pause-until(file),
sleep(ms). Config e.g. `BEAD_FAILPOINTS=push.before=exit(98)`. The grammar is `point=action` for the
first hit, `point@nth(k)=action` for hit k, and `point@always=action` for every hit; retry-heavy tests
use `backoff@always=skip`. Failpoints are present in every build and inert without explicit
configuration, as decided in `docs/RUST-PORT.md` §10.

## Required new port coverage (outside the existing test inventory)

**S02: JSON integer compatibility.** Pin the Python oracle to its supported default decimal
conversion limit with `PYTHONINTMAXSTRDIGITS=4300` and assert
`sys.get_int_max_str_digits() == 4300`. Rust's parser and writer must accept integers through
4,300 decimal digits and reject longer tokens; a minus sign does not count as a digit. Integers
beyond fixed-width ranges remain exact digit strings, not floats. A future wider domain requires aligning every
supported reader first.

Add positive and negative 4,300/4,301-digit differential fixtures for parsing, writing, import
and journal replay, including an integer nested in `create.data.extra`. A 4,300-digit value must
survive Rust import/write and Python replay/export unchanged; a 4,301-digit import or write must
fail before any event is published or partially written. Replaying a planted over-limit event
must report the same scoped invalid-JSON problem as Python and leave valid neighbouring events
readable. Exercise direct writer inputs as well as parsed inputs so that constructing a value in
memory cannot bypass the limit. These are new tests, not part of the 86 methods above or the
458-method catalogue total.

**S06: retained infrastructure driver cutover.** Adapt `infra/drive.py`, `infra/roundtrip.py` and
`infra/compat_real.py` to select an executable explicitly (honouring `BEAD_UNDER_TEST` for the
local process and the drive's per-machine selections), independent of the checkout's Python
launcher. Before running scenarios, record each selected path and version and assert the expected
implementation/version; the cutover gate requires the intended Rust release on every participant.
Keep helper-script discovery anchored to the source tree. Acceptance
must prove local Rust selection while `./bead` still exists and successful driver launches after
the launcher is removed; the four-machine drive must preflight every machine. Adapt the retained
roundtrip/check-runner tests to include preflight failures without weakening their existing exit
status and diagnostic assertions. These harness additions are required by the cutover gate;
they are not claimed as existing tests or as real-machine runs already performed.
