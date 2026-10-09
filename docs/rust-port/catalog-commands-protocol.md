<!-- Port catalogue for docs/RUST-PORT.md. Generated on 2026-10-09 by reading the Python tests at commit 9867715.
     Each row is a starting classification: confirm it when the test is ported, and correct it here if it is wrong. -->

# Test catalog for the Rust port: tests/test_commands.py and tests/test_protocol.py

Source: `tests/test_commands.py` (111 tests, 15 classes) and `tests/test_protocol.py` (54 tests, 8 classes), read in full, plus the package code they reach.

## Legend

**Mechanism**
- `cli`: runs `bead` as a subprocess (`Machine.run/bead`, or `subprocess.run([BEAD, ...])`) and inspects stdout, stderr, exit status, store files and git refs. "+ store tamper" means the test edits or deletes store files between runs.
- `inproc`: imports the package (`support.load_module()`) and calls its functions.
- `patched`: uses `support.patched(name, ...)` or `mock.patch*` in-process. `patched(env)` means only `os.environ` is patched.
- `driver`: spawns `python <script>` that loads the launcher with `runpy`/`SourceFileLoader`, monkeypatches it, then runs `main()`.
- `env-fake`: a fault or fake in the environment. Examples: a fake `git` on PATH, `GIT_EXEC_PATH`, a server hook in the bare remote, chmod, a closed pipe, an rlimit, SIGKILL, a small `BEAD_NET_TIMEOUT`, a renamed remote, or a `BEAD_TEST_CRASH_*` crash hook.

**Port class**
- `A`: black-box. Runs unchanged against a Rust binary via `BEAD_UNDER_TEST`, given the CLI contract, the documented knobs (`BEAD_HOME`, `BEAD_WORKSPACE`, `BEAD_AGENT`, `BEAD_THREAD`, `BEAD_ROTATE_BYTES`, `BEAD_NET_TIMEOUT`, `BEAD_REPACK_LOOSE`) and the stated constraints:
  - `git-via-PATH`: bead runs the git CLI found on PATH, passes its environment through, and gives each git call its own session through `setsid`.
  - `layout`: the on-disk store format 1.
  - `sync.git`: the internals of the private bare repo.
- `B`: pure in-process logic. Becomes a Rust unit or library test.
- `C-env`: needs a seam in the Rust code that the environment can trigger. Once Rust implements the hook, the test stays black-box and runs unchanged via `BEAD_UNDER_TEST`.
- `C-api`: needs an in-process injection seam such as a trait, a callback or a library API. The test must be rewritten in Rust.
- `D`: Python-specific; the column says what to test in Rust instead.

**Py-only assertions in A tests:** some A tests assert that stderr contains no `Traceback` or `BrokenPipeError`, or set `PYTHONUTF8=0`/`PYTHONCOERCECLOCALE=0`. In Rust, assert no `panicked at` and an exit status other than 101. The PYTHON* variables are harmless no-ops.

## tests/test_commands.py (111)

| class.method | subject (≤12 words) | mechanism | seams/fakes needed | port |
|---|---|---|---|---|
| BpTest.test_worktrees_and_clones_share_one_tracker_and_checkouts_stay_clean | Worktrees and clones share one tracker; checkouts stay clean | cli | none (git worktree/clone in sandbox) | A |
| BpTest.test_parallel_writers_lose_nothing_and_claim_is_exclusive | 30 concurrent creates lose nothing; racing claims: exactly one wins | cli (32 procs, 8 concurrent) | store lock across processes; real push-race backoff | A |
| BpTest.test_readiness_follows_the_rule_br_uses | Readiness rules: epics wait, blockers hold subtree; CYCLE | cli | none | A |
| BpTest.test_reads_work_when_the_store_is_read_only | Reads work on read-only BEAD_HOME; writes fail | cli+env-fake (chmod -R a-w) | read path creates/locks nothing; not run as root | A |
| BpTest.test_two_machines_converge_and_flag_what_both_changed | Machines converge; concurrent field edits flagged with both values | cli | none | A |
| BpTest.test_an_edit_made_after_seeing_another_is_not_a_conflict | Causally later edit wins without conflict | cli | none | A |
| BpTest.test_removing_a_label_only_removes_what_the_remover_saw | Label removal removes only observed additions | cli | none | A |
| BpTest.test_status_disagreement_holds_dependents_until_settled | Disputed status holds dependents, refuses claim until settled | cli | none | A |
| BpTest.test_same_child_number_on_two_machines_is_renumbered_identically | Colliding offline child ids renumbered identically on sync | cli | none | A |
| BpTest.test_cycle_formed_by_a_merge_is_reported_and_never_ready | Cycle formed by merge reported; nothing ready | cli | none | A |
| BpTest.test_concurrent_sync_from_three_machines_all_succeed | Three simultaneous syncs succeed and converge | cli (3 concurrent) | real push-race backoff | A |
| BpTest.test_online_claim_is_exclusive_across_machines | Online claim exclusive; simultaneous in_progress updates: one wins | cli (2 concurrent) | none | A |
| BpTest.test_claim_lasts_until_release_close_or_takeover | Claim guards, release, generation bump; close clears claims | cli | none | A |
| BpTest.test_takeover_fences_the_previous_owner | Takeover needs reason; old owner's closes become stale | cli | per-call BEAD_THREAD | A |
| BpTest.test_offline_claims_are_provisional | Offline claims provisional; never displace a confirmed owner | cli | none | A |
| BpTest.test_lost_acknowledgement_is_recovered_on_retry | Lost push acknowledgement: retry finds claim, no duplicate | cli+env-fake | crash-after-push: `BEAD_TEST_CRASH_AFTER_PUSH` → exit 99 | C-env |
| BpTest.test_local_work_after_a_lost_acknowledgement_does_not_fork_the_journal | After lost ack: claim unconfirmed, later work does not fork | cli+env-fake | crash-after-push (99); claims state "unconfirmed claim" | C-env |
| BpTest.test_a_refused_claim_leaves_no_trace_anywhere | Losing racing claim leaves no journal trace | cli (2 concurrent) | none | A |
| BpTest.test_an_interrupted_append_is_repaired_before_the_next_write | Torn segment tail skipped, repaired, fragment kept | cli + store tamper | layout: events/<r>/NNNNNN.jsonl, `*.torn-*` sibling | A |
| BpTest.test_a_crash_before_the_claim_is_written_leaves_no_false_owner | Crash before push: no owner; another machine claims | cli+env-fake | crash-before-push: `BEAD_TEST_CRASH_BEFORE_PUSH` → exit 98 | C-env |
| BpTest.test_a_sync_that_fetched_before_a_lost_acknowledgement_does_not_void_it | Sync with stale fetch keeps concurrently accepted claim | cli+env-fake (sh `git` on PATH: fetch sleeps 2s, then `exec /usr/bin/git`), 2 concurrent | git-via-PATH; crash-after-push; re-fetch under lock when pending exists | C-env |
| BpTest.test_closing_through_update_publishes_like_close | update --status closed publishes and ends claim | cli | none | A |
| BpTest.test_a_parent_change_cannot_form_a_loop | Parent loops refused with CYCLE | cli | none | A |
| BpTest.test_damaged_lines_and_payloads_are_reported_not_fatal | Non-JSON segment line reported, not fatal | cli + store tamper | layout | A |
| BpTest.test_events_record_the_branch_and_a_close_knows_whether_it_is_merged | Events record branch and commit; close knows merged state | cli + git worktree/commit/merge | none | A |
| BpTest.test_where_is_absent_outside_a_checkout_and_when_switched_off | No "where" outside a checkout or with where:false | cli | BEAD_WORKSPACE; `.beadroll.json` | A |
| BpTest.test_claim_needs_the_remote_unless_offline_is_asked_for | Unreachable --remote fails claim; --offline gives provisional | cli | none | A |
| BpTest.test_every_event_carries_machine_agent_and_thread | Events carry machine, agent, thread; ids are UUIDv7 | cli | BEAD_AGENT/BEAD_THREAD | A |
| BpTest.test_one_corrupt_event_does_not_stop_the_tracker | Conflicting same-id event quarantined; rest keeps working | cli + store tamper | layout (writes events/ffffffff/000001.jsonl) | A |
| BpTest.test_a_copied_machine_identity_is_detected | Copied machine home detected as JOURNAL_FORK | cli + copytree(home) | layout ($BEAD_HOME/replica.json) | A |
| BpTest.test_full_segments_are_never_rewritten | Full segments keep their blob ids on the remote | cli + `git ls-tree` on remote | knob BEAD_ROTATE_BYTES=1500; remote tree layout | A |
| BpTest.test_tracked_workspace_id_and_a_separate_coordination_remote | Tracked workspace id, coordination remote, foreign workspace refused | cli + git inspection | none | A |
| BpTest.test_replay_does_not_depend_on_delivery_order | Replay identical under any delivery order and duplication | cli setup + inproc | lib API: Workspace, load_journal, Journal.add(sequenced=), .order/.events, replay, export_records, detail, replica_id | B |
| BpTest.test_import_export_round_trips_beads_records | Beads JSONL round-trip; comment ids; idempotent reimport | cli | none | A |
| CompatAuditCLI.test_global_json_actor_positions | --json and --actor accepted in any position | cli | none | A |
| CompatAuditCLI.test_description_stdin_multi_id_and_filters | Stdin description, multi-id update/show, list filters | cli (stdin) | none | A |
| CompatAuditCLI.test_multi_id_update_rolls_back_on_missing_issue | Multi-id update is atomic when one id is missing | cli | none | A |
| CompatAuditCLI.test_domain_error_json_envelope | Domain error: exit 1, empty stdout, JSON envelope | cli | none | A |
| CompatAuditCLI.test_parser_error_json_envelope | Parser error: exit 2, JSON envelope on stderr | cli | none | A |
| CompatAuditCLI.test_global_options_respect_end_of_options | Arguments after -- are data, never options | cli | none | A |
| CompatAuditCLI.test_claiming_update_validates_parent_before_committing_claim | Invalid parent refuses claiming update; no claim left | cli | none | A |
| CompatAuditCLI.test_workspace_config_cannot_escape_home | Path-like workspace ids rejected with CONFIG | cli | none | A |
| CompatAuditCLI.test_changed_workspace_config_does_not_delete_old_store | Switching workspace config leaves old store intact | cli | none | A |
| CompatAuditCLI.test_import_preserves_dependency_metadata_extra_and_tombstone_mutation | Import keeps extra fields, dependency metadata; delete tombstones | cli | none | A |
| CompatAuditCLI.test_imported_comment_zero_id_roundtrips | Imported comment id 0 survives export | cli | none | A |
| CompatAuditCLI.test_metadata_excludes_local_machine_identifiers | Event metadata has no paths, hostname or username | cli + reads store | layout; replica id `^[0-9a-f]{8}$` | A |
| CompatAuditIdentity.test_detect_actor_environment_and_explicit_overrides | Actor detection from env: human, claude, codex, BEAD_* overrides | inproc+patched(env) | detect_actor as a pure function of (env map, name) | B |
| CompatAuditIdentity.test_nearest_harness_mocked_process_tree | Nested harnesses resolved by nearest ancestor process | inproc+patched (`mock.patch.object(subprocess,'run')` fakes `ps` output) | process-tree lookup injection (`ps -o ppid=,args= -p PID`, at most 12 levels) | C-api (or C-env with a fake `ps` on PATH, if Rust still shells out to ps) |
| CompatAuditIdentity.test_missing_ps_does_not_crash_nested_actor_detection | Missing ps: falls back to codex, no crash | cli+env-fake (PATH holds only python3 and git) | process lookup fails soft; `info` outside a repo with BEAD_WORKSPACE | A |
| CompatAuditIdentity.test_bp_thread_overrides_detected_thread_without_bp_agent | BEAD_THREAD overrides the detected thread | inproc+patched(env) | detect_actor(env) | B |
| LocationReview.test_claims_and_log_report_claim_location_and_later_activity | Claims/log report claim branch, commit and later activity | cli + git worktree | ms-precision event times (last_activity compared as strings) | A |
| CommandsAndCompatibilityReview4.test_q05_tombstone_update_cannot_leave_a_releasable_claim | Claim+tombstone update refused cleanly or unreleasable | cli + store listing | layout: pending/*.json, events/*/claims/*.json | A |
| CommandsAndCompatibilityReview4.test_q11_offline_close_ends_a_provisional_claim | Offline close clears seen provisional claims, keeps concurrent | cli | none | A |
| CommandsAndCompatibilityReview4.test_q06_incremental_import_resolves_late_dependency_target | Split import equals one-shot import in deps and readiness | cli | none | A |
| CommandsAndCompatibilityReview4.test_q10_reserved_dangling_extra_cannot_crash_export | Unknown `_dangling_dependencies` field kept; export works | cli | none | A |
| DeepTreeReview4.test_q08_deep_valid_dependency_tree_does_not_overflow | dep tree over a 1,100-deep chain does not overflow | inproc fixture (`append_events` under patched(env)) + cli | fixture writer: replace with writing a format-1 segment directly; BEAD_WORKSPACE; cwd not set (runner's cwd) | A (after fixture rewrite) |
| Review5CLI.test_e01_human_output_escapes_terminal_controls | Human output escapes ESC, BEL and newline from titles | cli | none | A |
| Review5CLI.test_e02_remote_credentials_are_not_disclosed_or_world_readable | Remote password redacted; meta.json owner-only | cli + file mode | layout (meta.json) | A |
| Review5CLI.test_e03_takeover_fences_an_owners_concurrent_offline_delete | Takeover fences owner's concurrent offline delete | cli | none | A |
| Review5CLI.test_e04_invalid_import_fails_instead_of_counting_an_inert_event | Invalid status import fails VALIDATION; no integrity residue | cli | none | A |
| Review5CLI.test_e05_silent_create_still_honors_json_mode | create --silent --json still emits JSON | cli | none | A |
| Review5CLI.test_e06_export_never_emits_nonfinite_json_numbers | NaN import refused or never exported | cli | none | A |
| Review5CLI.test_e07_closed_downstream_pipe_has_defined_quiet_exit | Reader closes stdout early: quiet exit 0 | cli+env-fake (stdout pipe closed) | handle EPIPE; Py-only strings | A |
| Review5CLI.test_e08_nullable_import_values_do_not_crash_render_or_export | Null comment/dependency fields render and export | cli | none | A |
| Review5CLI.test_e09_invalid_config_types_return_structured_config_errors | Wrong-typed .beadroll.json → CONFIG error | cli | Py-only Traceback check | A |
| Review5CLI.test_e11_parent_filter_matches_every_direct_parent | list --parent matches each of several parents | cli | none | A |
| Review5CLI.test_e12_ascii_locale_cannot_turn_a_committed_create_into_failure | C locale cannot fail a committed create | cli (LC_ALL=C) | PYTHON* vars moot | A |
| Review5CLI.test_e13_receipt_identity_includes_resolved_file_content | Retry with changed file content is a new request | cli+env-fake | crash-after-push; receipt signature includes resolved file content | C-env |
| Review5CLI.test_v01_machine_json_preserves_and_escapes_control_data | JSON escapes controls and round-trips the title | cli | none | A |
| Review5PositiveControls.test_v02_help_needs_no_repository_and_creates_no_state | --help outside a repo creates no state | cli | env inherits the runner's environment unfiltered | A |
| HelpAndPrime.test_no_arguments_and_help_print_the_overview_and_exit_zero | No args/--help/-h/help print overview, exit 0 | cli | exact overview section names | A |
| HelpAndPrime.test_help_command_shows_one_commands_usage | help COMMAND usage; prime text; unknown command USAGE | cli | argparse-style `usage: bead claim` | A |
| HelpAndPrime.test_every_command_has_help | Every subcommand's --help shows `usage: bead NAME` | inproc (`build_parser()._actions` introspection) + cli | subcommand enumeration | D → a Rust unit test iterates `Cli::command().get_subcommands()`; the integration test loops over the 27 names |
| HelpAndPrime.test_prime_and_onboard_work_with_and_without_a_tracker | prime/onboard outside, before and after init; matches AGENTS.md | cli + reads AGENTS.md | AGENTS.md is located at `Path(BEAD).resolve().parent`, which breaks under BEAD_UNDER_TEST | A (fix the path) |
| Review6.test_f01_failed_command_does_not_forget_an_accepted_retry | Unrelated failed command keeps receipt; retry emits once | cli+env-fake | crash-after-push; layout pending/*.command | C-env |
| Review6.test_f02_locale_retry_keeps_the_accepted_description | Retry under Latin-1/C locale decodes file as UTF-8 | cli+env-fake (locale) | crash-after-push; LC_ALL/LANG | C-env |
| Review6.test_f03_credential_urls_are_not_stored | Uppercase, slash and token credentials never stored or shown | cli | none | A |
| Review6.test_f04_checkout_files_stay_readable_in_a_shared_tree | Checkout files follow umask (group-readable) | cli + file mode | runner umask 022 | A |
| Review6.test_f05_human_output_escapes_tab_and_line_separators | Human output escapes TAB, U+2028, U+202E | cli | none | A |
| Review6.test_f06_failed_command_with_closed_stderr_keeps_its_status | Closed stderr on failed command keeps exit 1 | cli+env-fake (stderr pipe closed) | stderr EPIPE must not panic (eprintln! would exit 101) | A |
| Review6.test_f07_non_utf8_input_is_a_structured_error | Non-UTF-8 argv or file → VALIDATION; nothing stored | cli (raw 0xFF byte in argv) | read argv as bytes (args_os) | A |
| Review6.test_f08_import_probes_dependencies_before_writing | Malformed import dependencies fail before any write | cli | none | A |
| Review6.test_f09_dangling_dependency_can_be_retracted_before_its_target_exists | Retracted dangling dependency stays gone after its target arrives | cli + inproc | lib API: Journal.add, replay, State.issues[uid]["deps"], is_blocked | A+B |
| Review6.test_f10_non_event_json_makes_the_whole_attempt_inert | Non-event JSON line makes a whole attempt inert | cli + store tamper | layout events/*/claims/*.json | A |
| Review6.test_f11_stale_pending_twin_does_not_fork_a_matching_attempt | Pending twin of a confirmed attempt is not ATTEMPT_FORK | cli + store tamper | layout pending/<id>.json | A |
| Review6.test_f12_unreadable_pending_file_is_not_fenced_forever | Junk pending file removed; at most one fence commit | cli + store tamper + `git rev-list` | layout pending/ | A |
| Review6.test_f13_local_state_files_are_strict_json | NaN or wrong-shaped meta/replica/aliases files handled | cli + store tamper | layout meta.json, replica.json, aliases.json | A |
| Review6.test_f14_future_clock_create_is_a_structured_error | Clock beyond 2100 gives structured create error | driver (`time.time_ns = lambda: 4.429e18`) + cli | clock injection (absolute) | C-env (e.g. a `BEAD_TEST_NOW_MS` hook) |
| Review6.test_f15_timeout_reaps_the_git_helper | Fetch timeout kills git-upload-pack too | cli+env-fake (`GIT_EXEC_PATH` dir with sleeping upload/receive-pack, BEAD_NET_TIMEOUT=2) | git-via-PATH; killpg on fetch timeout; env passthrough | A |
| Review6.test_f16_read_only_store_write_is_a_structured_error | Write to a read-only store is a structured error | cli+env-fake (chmod) | not root | A |
| Review6.test_receipt_identity_holds_for_actor_sync_stdin_and_json | Receipt survives other threads and sync; stdin and --json handled | cli+env-fake (stdin) | crash-after-push; layout pending/*.command | C-env |
| Review6.test_remote_url_shapes_that_are_already_safe | Encoded, ssh, scp and named remote URLs stored safely | cli + reads meta.json | layout meta.json | A |
| Review6.test_prime_matches_ready_and_help_outside_a_repository_is_silent | prime's ready count matches; help/prime/onboard stateless | cli | none | A |
| Review6.test_incremental_mutual_import_matches_one_shot | Mutual dependency imported in two batches equals one-shot | cli | none | A |
| Review6.test_clock_skew_does_not_override_causality | Clock 48h behind: causality, not time, settles | driver (`time.time_ns` offset) + cli | clock injection (offset) | C-env (e.g. a `BEAD_TEST_CLOCK_OFFSET_MS` hook) |
| Review6.test_closed_stdout_on_success_exits_zero_and_store_stays_private | Closed stdout after a successful read exits 0 | cli+env-fake (closed pipe) | EPIPE → exactly 0 (dying by SIGPIPE is not accepted) | A |
| Review6.test_json_keeps_bidi_text | JSON preserves bidi characters exactly | cli | none | A |
| FormatPolicy.test_unapplicable_events_travel_untouched_and_hold_nothing_up | Unknown-op/v2 events kept byte-for-byte and block nothing | cli + store tamper + inproc (`uuid7` only) | test-side UUIDv7 generator; layout | A |
| SyncRepoDamage.test_sync_rebuilds_a_damaged_sync_repo_once_and_info_lists_the_copy | Damaged sync.git set aside, rebuilt once, listed | cli + sync.git tamper | sync.git: refs/beads/remote, head commit is a loose object, `sync.git.damaged-<ns>` | A |
| SyncRepoDamage.test_a_claim_rebuilds_too_and_damage_twice_in_one_command_is_reported | Claim path rebuilds sync.git; local-damage classifier | cli + inproc (`local_damage`) | sync.git; pure classifier over git stderr | A+B |
| SyncRepoMaintenance.test_no_git_maintenance_in_sync_git_and_fetches_stay_packed | sync.git auto-gc off; each fetch adds one pack | inproc+patched(env, chdir) (`Link`, `find_workspace`, `run_git`, `SYNC_REPO_CONFIG`) + cli | the Link's git env (GIT_CONFIG_KEY/VALUE_n) must be observable; pack counting | B+A |
| SyncRepoMaintenance.test_repacks_under_the_lock_when_loose_objects_pile_up | Repacks when loose objects exceed threshold; fsck clean | cli + sync.git inspection | knob BEAD_REPACK_LOOSE=3; sync.git | A |
| PublishOnWrite.test_writes_reach_the_other_machine_without_a_sync | Every local write publishes at once | cli | none | A |
| PublishOnWrite.test_offline_keeps_it_local_and_says_so_until_the_next_publish | --offline stays local; unpublished count reported | cli | exact human line | A |
| PublishOnWrite.test_an_unreachable_remote_never_fails_a_write | Unreachable remote: write succeeds, reported unpublished | cli+env-fake (bare remote renamed away) | none | A |
| PublishOnWrite.test_init_with_an_import_publishes_the_tracker_at_once | init --import publishes at once; init --offline works | cli | none | A |
| ReadySubtree.test_j01_the_subtree_at_any_depth_judged_on_the_whole_tracker | ready ID: recursive subtree, global readiness, order, filters | cli | exact ready order; human row format | A |
| ReadySubtree.test_j01_claims_and_disputed_statuses_count_inside_as_everywhere | Claims and status conflicts exclude issues inside subtree | cli | none | A |
| ReadySubtree.test_j01_a_loop_through_the_root_and_a_chain_deeper_than_the_recursion_limit | Subtree walk survives parent loops and 1,100-deep chain | cli | iterative walk (no stack overflow) | A |
| ReadySubtree.test_j02_a_scope_that_names_no_issue_is_an_error_whatever_else_filters | Unknown scope id → ISSUE_NOT_FOUND whatever the filters | cli (14 runs) | none | A |
| ReadySubtree.test_j03_one_scope_at_a_time | Two scopes → USAGE, exit 2; help explains scopes | cli | exact help phrases | A |

## tests/test_protocol.py (54)

| class.method | subject (≤12 words) | mechanism | seams/fakes needed | port |
|---|---|---|---|---|
| SyncAudit.test_torn_tail_reader_ignores_only_unterminated_fragment | Reader skips only the unterminated trailing fragment | cli + store tamper | layout | A |
| SyncAudit.test_bug_torn_tail_followed_by_successful_append_must_stay_readable | Append after torn tail keeps journal readable | cli + store tamper | layout | A |
| SyncAudit.test_bug_file_size_limit_partial_append_must_not_poison_next_write | Partial append under RLIMIT_FSIZE repaired by next write | cli+env-fake (setrlimit FSIZE in preexec) | append goes straight to the segment (size == limit); Rust should ignore SIGXFSZ as CPython does | A |
| SyncAudit.test_bug_foreign_fork_report_must_not_replace_published_journal | Tampered foreign segment never overwrites remote copy | cli + store tamper + `git show` | layout; `forked_journals` holds the path | A |
| SyncAudit.test_bug_copied_machine_id_fork_across_segments_is_detected | Copied identity forking across segments → JOURNAL_FORK | cli + copytree(home) | knob BEAD_ROTATE_BYTES (1 vs 1000000) via Machine.extra | A |
| SyncAudit.test_bug_segment_boundary_fork_must_not_erase_concurrent_title | Segment order never invents causality | cli + copytree(home) | knob BEAD_ROTATE_BYTES | A |
| SyncAudit.test_bug_kill_mid_append_then_successful_write_must_stay_readable | SIGKILL mid-append: readers unaffected, next write repairs | driver (`os.fdopen` 'ab' writes 100 bytes, marker, sleep 20) + env-fake (killpg) + cli | pause after a partial append | C-env (e.g. a `BEAD_TEST_HANG_MID_APPEND=<marker>` hook) |
| SyncAudit.test_slow_push_holds_lock_then_local_writer_completes_without_loss | Writer waits for lock held by slow push; nothing lost | cli+env-fake (python `git` on PATH: real push, marker, sleep 20; killpg) | git-via-PATH; lock held across publish; flock freed on kill | A |
| SyncAudit.test_parallel_rotation_preserves_causal_predecessor_and_all_events | 50 parallel creates with rotation form one causal chain | cli (50 procs, 12 concurrent) | knob BEAD_ROTATE_BYTES=600; layout | A |
| SyncAudit.test_deleted_local_segment_is_restored_from_remote | Deleted local segment restored from remote | cli + store tamper | knob BEAD_ROTATE_BYTES=200 | A |
| SyncAudit.test_remote_rollback_and_deleted_ref_restore_from_surviving_copy | Rolled-back or deleted remote ref republished | cli + `git update-ref` on remote | none | A |
| SyncAudit.test_claim_lost_ack_after_real_push_kill_is_recovered | Claim killed after real push: retry recovers one claim | cli+env-fake (push wrapper, killpg at marker) | git-via-PATH | A |
| SyncAudit.test_bug_local_write_after_lost_ack_must_not_fork_accepted_claim | Write after lost ack does not fork accepted claim | cli+env-fake | crash-after-push (99) | C-env |
| SyncAudit.test_claim_timeout_after_real_applied_push_is_recovered | Push applied but timed out: claim still recovered | cli+env-fake (push wrapper sleeps 20; BEAD_NET_TIMEOUT=0.4) | git-via-PATH; push timeout kills only the client → uncertain | A |
| SyncAudit.test_killed_slow_fetch_does_not_block_local_writer | Stalled fetch holds no lock; writer proceeds | cli+env-fake (fetch wrapper; killpg) | git-via-PATH; first fetch outside the store lock | A |
| SyncAudit.test_manifest_missing_and_foreign_workspace_are_refused | Ref without manifest, or another workspace's, refused | cli + `git update-ref` | BEAD_WORKSPACE | A |
| SyncAudit.test_retry_exhaustion_retains_local_edits_and_no_partial_claim | 12 rejected pushes → SYNC_CONFLICT; nothing partial | env-fake (hostile `git` advances the ref before every push) + driver (`time.sleep` no-op) + cli | git-via-PATH; backoff-disable seam; exactly 12 attempts per command | C-env (`BEAD_FAILPOINTS=backoff@always=skip`) |
| RootAudit.test_six_simultaneous_claims_have_one_winner | Six simultaneous claims: one winner; all converge | cli (6 concurrent) | none | A |
| RootAudit.test_claim_racing_release_matches_accepted_outcome | Claim racing release ends consistent with the accepted outcome | cli (2 concurrent) | none | A |
| RootAudit.test_two_takeovers_are_serial_and_generation_increases | Concurrent takeovers serialize; generations 2 and 3 | cli (2 concurrent) | none | A |
| RootAudit.test_takeover_racing_release_ends_at_new_owner | Takeover racing release ends with the new owner | cli (2 concurrent) | none | A |
| RootAudit.test_forced_close_tombstone_and_reopen_generation | Forced close, reopen, delete: claim validity and generations | cli | none | A |
| RootAudit.test_provisional_release_and_same_thread_online_upgrade | Provisional release; same-thread online claim replaces provisional | cli | none | A |
| RootAudit.test_nonowner_cannot_bypass_close_guard_with_status_update | Non-owner cannot close through update --status closed | cli | per-call BEAD_THREAD | A |
| RootAudit.test_claimed_issue_is_not_ready_after_nonowner_status_update | Claimed issue stays unready after a non-owner status edit | cli | none | A |
| RootAudit.test_clock_moves_back_but_later_observed_edit_settles_register | Clock a day back: later observed edit still wins | driver (SourceFileLoader; `bead.time.time_ns` offset) + cli | clock injection (offset) | C-env |
| ProtocolReaudit.test_refused_void_publish_must_not_activate_unaccepted_claim | Losing fence race never activates an unaccepted claim | inproc+patched (`Link.publish` raises, then a race wrapper runs b's claim; `pause`) + cli | publish fault; pre-publish interleave hook; pause hook | C-api (black-box alternative: CRASH_BEFORE_PUSH plus a PATH `git` that runs b's claim before the first push) |
| ProtocolReaudit.test_failed_void_publish_retry_must_not_activate_unaccepted_claim | Failed fence publish; a later claim elsewhere wins | inproc+patched (`Link.publish` raises REMOTE_UNREACHABLE) + cli | publish fault injection | C-api (alternative: PATH `git` failing push with no status line) |
| ProtocolReaudit.test_failed_void_push_must_leave_original_claim_unconfirmed | Failed fence leaves the claim unconfirmed | inproc+patched + cli | publish fault injection | C-api |
| ProtocolReaudit.test_failed_multievent_update_void_retry_must_not_commit_edits | Unaccepted claiming update's bundled edits never land | inproc+patched + cli | publish fault injection | C-api |
| ProtocolReaudit.test_quiet_reconcile_of_unaccepted_claim_preserves_independent_work | Unaccepted claim dropped quietly; independent comment kept | inproc+patched (interrupted claim only) + cli | publish fault (CRASH_BEFORE_PUSH would do) | C-api (C-env feasible) |
| ProtocolReaudit.test_multievent_update_lost_ack_applies_whole_transaction | Lost-ack claiming update lands as one transaction | cli+env-fake | crash-after-push | C-env |
| ProtocolReaudit.test_nonzero_applied_push_with_interleaved_writer_must_not_fork | Applied push reported as failed, with a writer between retries: no fork | inproc+patched (`run_git` lose-ack; `pause` runs a CLI writer) + cli | git-runner injection; pause callback | C-api |
| ProtocolReaudit.test_delayed_push_after_timeout_and_absent_fetch_must_not_fork | Push landing after timeout: attempt settles, no fork | cli+env-fake (pre-receive gate ≤20s; BEAD_NET_TIMEOUT=0.3) | push timeout kills only the client; reads pending/*.json | A |
| ProtocolReaudit.test_successful_push_with_nonzero_exit_must_preserve_accepted_claim | Push status line outranks a nonzero exit | cli+env-fake (sh `git`: real push, then `exit 1`) | git-via-PATH; outcome from the porcelain status line | A |
| RootReaudit.test_importing_legacy_comment_does_not_renumber_existing_local_comment | Legacy comment import keeps local comment ids | cli | none | A |
| RootReaudit.test_forced_close_through_update_publishes_like_forced_close | update --status closed --force publishes | cli | none | A |
| RootReaudit.test_claim_update_cycle_validation_is_atomic_for_all_targets | Cycle in multi-id claiming update: nothing written or claimed | cli | none | A |
| RootReaudit.test_failed_claim_does_not_publish_bundled_field_edit | Refused claim publishes no bundled edit | cli | none | A |
| Protocol3.test_fence_wins_against_a_real_push_still_running_after_timeout | Fence beats a stalled in-flight push; claim never lands | inproc+patched (`NET_TIMEOUT`=.25) + env-fake (pre-receive gate) + cli | lib API: locked, Link.fetch/merge/publish → ok/rejected/refused/uncertain, Tx, claim_builder, write_attempt_file; per-Link timeout | C-api |
| Protocol3.test_reader_sees_all_or_none_of_accepted_claim_and_bundled_edits | Reader sees an accepted attempt all or nothing | inproc+patched (`write_atomic` wrapper runs a CLI reader around claims/ write) + cli | hook around the confirmation write | C-api |
| Protocol3.test_kill_during_local_confirmation_does_not_leave_partial_atomic_update | SIGKILL before confirm rename: no partial update; recovers | driver (`os.replace` spins for claims/ target) + env-fake (kill) + cli | pause before the attempt file is renamed into claims/; readers ignore temp files | C-env (e.g. a `BEAD_TEST_HANG_BEFORE_CONFIRM=<marker>` hook) |
| Protocol3.test_recovered_atomic_update_is_not_applied_again_after_a_later_observed_edit | Recovered update is not reapplied over a later edit | inproc+patched (`run_git` lose-ack; `pause` runs CLI sync and update) + cli | git-runner injection; pause callback | C-api |
| Protocol3.test_release_recovers_a_successful_push_with_lost_acknowledgement | Release with lost ack recovered and reported as success | inproc+patched (`run_git` lose-ack; `pause` no-op) + cli | git-runner injection | C-api (alternative: PATH `git` doing `push >/dev/null; exit 1` once) |
| Protocol3.test_mixed_ownership_file_and_journal_ancestry_across_six_machines | Ancestry and replay stable across six-machine shuffles | inproc | lib API: Journal.add(sequenced=), replay, State.saw/integrity/pending/issues, detail; seeded RNG 3106 | B |
| Protocol3.test_rejected_fences_leave_attempt_pending_and_excluded | Refused fence keeps the attempt pending and inert | cli+env-fake (CRASH_BEFORE_PUSH; pre-receive `exit 1`) + inproc (`synchronise`, patched `pause`) | crash-before-push; the inproc sync is replaceable by `bead sync --json` (REMOTE_REFUSED) | C-env |
| ActualTransportReview.test_timed_out_accepted_release_returns_success | Timed-out but accepted release returns success | cli+env-fake (pre-receive sleeps 0.15; BEAD_NET_TIMEOUT=0.1) | push timeout semantics | A |
| ActualTransportReview.test_crash_after_accepted_release_can_be_retried_idempotently | Retry after crash-after-push release succeeds | cli+env-fake | crash-after-push | C-env |
| ProtocolReview4.test_q01_damaged_immutable_attempt_cannot_replace_remote_transaction | Truncated local attempt never replaces the remote one | cli + store tamper + `git show` | layout claims/ | A |
| ProtocolReview4.test_q01_malformed_attempt_is_inert_as_a_whole | Malformed attempt file wholly inert and reported | cli + store tamper | layout events/<r>/claims/<id>.json | A |
| ProtocolReview4.test_q02_retry_after_accepted_claiming_parent_move_keeps_parent | Retry of accepted claiming parent move keeps new parent | cli+env-fake | crash-after-push | C-env |
| ProtocolReview4.test_q02_retry_after_accepted_claiming_close_emits_one_close | Retry of accepted claiming close emits one close | cli+env-fake | crash-after-push | C-env |
| ProtocolReview4.test_q03_crash_after_confirmation_before_cleanup_is_idempotent | Crash after confirm, before pending cleanup: retry idempotent | inproc+patched (`drop_pending` raises; env, chdir) + cli | crash between confirm_ownership and drop_pending | C-api (C-env hook feasible) |
| ProtocolReview4.test_q04_future_manifest_is_refused_without_remote_rewrite | Future-format manifest refused; remote untouched | cli + attacker push | none | A |

## Totals

| | test_commands (111) | test_protocol (54) |
|---|---|---|
| A | 95 | 32 |
| B | 4 | 1 |
| C-env | 10 | 10 |
| C-api | 1 | 11 |
| D | 1 | 0 |
| uses cli | 108 | 53 |
| uses inproc | 10 | 13 |
| uses patched | 5 (4 only patch os.environ) | 12 |
| uses driver | 2 | 4 |
| uses env-fake | 16 (8 are BEAD_TEST_CRASH_*) | 18 (6 are BEAD_TEST_CRASH_*) |
| cli only | 83 | 24 |

Secondary classes: f09 and SyncRepoDamage-claim are A+B; SyncRepoMaintenance-1 is B+A; DeepTree is A once its fixture is written directly.

## Notes for the porter

- **BEAD_UNDER_TEST caveats.**
  - The 6 driver tests (f14, clock_skew, kill_mid_append, retry_exhaustion, clock_moves_back, kill_during_confirmation) `runpy` the launcher, so they cannot target a binary.
  - `load_module()` tests always exercise the Python package. When `BEAD_UNDER_TEST` is set, the mixed ones (ProtocolReaudit, Protocol3, q03, replay order, SyncRepoMaintenance-1) run Python code against a store or remote that the Rust binary wrote. They become format-compatibility checks, not Rust tests.
  - prime/onboard reads `AGENTS.md` beside the resolved `BEAD`.
  - The slow-git wrapper hard-codes `/usr/bin/git`.
  - The missing-ps PATH ships `python3`, which only the Python launcher needs.
- **Script mode skips five classes.** `if __name__ == "__main__": unittest.main()` sits before `FormatPolicy` in test_commands.py, so running the file as a script skips the last 5 classes. `-m unittest` runs everything.
- **Signals in Rust.**
  - Rust ignores SIGPIPE, but `println!` and `eprintln!` panic on EPIPE. Use explicit writes: a closed stdout must exit exactly 0, and a failed command with a closed stderr must keep exit 1.
  - CPython ignores SIGXFSZ. Rust should too, so that EFBIG becomes a structured IO error.
- **Timeouts and process groups.** Every git child runs in its own session through a real `setsid` call in the audited pre-exec runner path. `process_group(0)` alone retains the controlling terminal, and must not precede `setsid` because a process-group leader cannot create a session. On a fetch timeout, kill the whole process group. On a push timeout, kill only the client, so receive-pack runs on and the outcome is "uncertain". Keep the controlling-terminal and group-kill timeout tests as runner acceptance checks.
- **Retry budget.** Retries are exactly `range(1, 13)` with `pause(U(0.1,0.4)*attempt)` after every failed round (sync, and each ownership command). Retry-heavy tests use `BEAD_FAILPOINTS=backoff@always=skip`; `point=action` fires on the first hit, `point@nth(k)=action` on hit k, and `point@always=action` on every hit. These hooks are present in every build and inert without explicit configuration.
- **Readers ignore in-flight files.** Readers and sync must ignore `.<name>.<pid>.tmp` temp files and non-`.json`/`.jsonl` files (e.g. `*.torn-*`, `*.fork-*`).

## Required new port coverage (outside the existing test inventory)

**S01: receipts across upgrades and rollback.** Keep Python's `command_signature` bytes exactly:
the same argument fields and defaults, actor and resolved file/stdin request, sorted keys, default
spaced separators and ASCII escaping; omit `func`, callables and `json` as Python does. An accepted
`.command` receipt can remain after `sync` reports no pending attempts, so sync is not a receipt
migration and accepted receipts must not be discarded during installation.

Add same-store Python → Rust → Python tests that exercise each transition with a newly accepted
but unacknowledged command. After the crash, another machine edits the issue; sync the first
machine, switch implementation while retaining its store and identity, and retry the identical
command as the same actor. The retry must preserve the intervening edit and append no second
attempt or command events. Exercise both a Python-written receipt consumed by Rust and a
Rust-written receipt consumed by Python, including resolved file/stdin inputs and `--json`
normalisation. These are new compatibility tests; the 165 existing methods above do not already
prove either transition.
