# Findings ledger

Every finding from every review, in one place. The reports themselves are kept verbatim under
`reviews/`; this file is the living record of what each finding became. A finding is **fixed**
when the behaviour is changed and a test in `tests/` fails without the fix; **by design** when
the behaviour is kept on purpose (with the reason); **open** otherwise.

Reviewers: propose new rows in your report using the next free id for your review (see
`REVIEWING.md`); the maintainer merges them here.

## Review 7 (`reviews/2026-10-07-7-claude.md`, ids G)

65 findings and a 3,300-schedule enumeration of the ownership protocol (which held, except G12).
Closed in three commits: security and git faithfulness, receipts and push outcomes, imports and robustness.

| Id | Severity | Finding | Status | Resolution | Tests |
|---|---|---|---|---|---|
| G01 | High | A tracked `remote` starting with `-` reached git as an option (`--upload-pack=<cmd>` ran a command) | fixed | `--` before the remote on fetch and push | `G01_TrackedRemoteIsNeverAGitOption` |
| G02 | High | A committed `.beadroll.json` could bind a checkout to another repository's URL-derived store and redirect an existing store's remote | fixed | A tracked id shaped like a derived id (16 hex / `local-…`) that is not this origin's is `CONFIG`; a tracked remote that differs from the one already in use is `CONFIG` unless named explicitly (`init --tracked --remote` records the switch) | `G02_TrackedConfigCannotAdoptAnotherRepositorysStore` |
| G03 | High | An unreadable `meta.json` silently moved coordination to origin (two confirmed owners; tracker pushed into the code origin) | fixed | `meta(strict=True)` for the remote decision: a damaged file is `IO`, never a fallback; a typed or tracked remote still works and repairs it; `save_meta` never overwrites a damaged file without a remote to put in it; `info` reports the damage | `G03_DamagedMetaMovesCoordinationToOrigin` |
| G04 | High | Re-running `init --tracked` rewrote `.beadroll.json` from scratch, dropping `ref`, `where` and unknown keys (two refs → two owners) | fixed | The rewrite starts from the existing config; a kept remote is written back without its password | `G04_TrackedInitKeepsHandSetConfig` |
| G05 | High | `host:path`, ssh aliases, `gh:` shortcuts, `[::1]:path`, upper-case schemes and `transport::` origins were joined onto the checkout as paths: one tracker per clone | fixed | `is_local_path` follows git's own rule; relative paths are resolved from the top; schemes normalise case-insensitively | `G05_OriginsGitReadsAsRemotes` |
| G06 | High | `https://TOKEN:x-oauth-basic@host` kept the token in `.beadroll.json`/`meta.json` while claiming the password was dropped | fixed | Over http(s) the whole user-info is treated as a secret | `G06_HttpsTokenInTheUserPosition` |
| G13 | Medium | `.beadroll.json` was decoded in the locale encoding | fixed | UTF-8 | `G13_TrackedConfigIsUtf8WhateverTheLocale` |
| G17 | Medium | A translated git's "couldn't find remote ref" was not recognised: first sync failed | fixed | `LC_ALL=C` for sync.git's git | `G17_TranslatedGit` |
| G19 | Medium | The user's client pre-push hooks and `init.templateDir` hooks ran inside sync.git | fixed | `git init --template=` and `git push --no-verify`; the remote's hooks still apply | `G19_ClientHooksOnPrivatePushes` |
| G20 | Medium | The checkout's transport config (`http.*.extraheader`, credential helpers, `core.sshCommand`, `includeIf`) did not reach sync.git; `GIT_SSH` was overridden | fixed | `transport_config()` replays the checkout's transport keys through `GIT_CONFIG_*`, resettable lists cleared first; the ssh timeout default applies only when nothing else is set | `G20_TransportSettings` |
| G21 | Medium | Remotes resolved against the cwd: a stored relative path failed from subdirectories, a same-named directory shadowed a configured remote (two owners) | fixed | A configured remote wins; stored relative paths resolve from the top; `init` stores typed paths relative to the top (tracked) or absolute (meta) | `G21_RemoteResolvedAgainstTheCwd` |
| G22 | Medium | origin was the *last* `remote.origin.url`, ignoring `pushurl` | fixed | First `pushurl`, else first `url`: where `git push origin` goes | `G22_OriginIsNotWhereGitPushes` |
| G23 | Medium | A tracked config was read from the working tree only: older branches, bisects and bare clones became other workspaces | fixed | `committed_config()` reads `HEAD:.beadroll.json`, then `origin/HEAD`'s, when the tree has none | `G23_TrackedConfigReadFromWorkingTreeOnly` |
| G24 | Medium | A renamed repository's URL-derived id changed: `WORKSPACE_MISMATCH` with no remedy | fixed | The error names the remedy; the first `sync` adopts the published workspace when the id was derived, the store is empty and the remote is this origin, remembered in `moved.json` | `G24_RenamedRepositoryLosesItsTracker` |
| G25 | Medium | The first remote used (origin included) was frozen as "last used" and outranked origin forever | fixed | origin is never remembered; a remembered value equal to origin is ignored | `G25_FirstSyncRemoteIsFrozen` |
| G26 | Medium | sync.git took this machine's default object format, not the project's; a wrong one stayed wrong | fixed | `--object-format=<project's>`; a sync.git in another format is remade | `G26_ObjectFormat` |
| G27 | Medium | An unfinished sync.git (empty dir, no `objects/`) was never completed; `GIT_WORK_TREE`/`GIT_OBJECT_DIRECTORY` leaked into it | fixed | `git init` whenever `HEAD`/`objects`/`refs` is missing, with every repository-location variable stripped | `G27_UnfinishedSyncGit` |

## Review 6 (`reviews/2026-10-07-6-grok.md`, ids F)

| Id | Severity | Finding | Status | Resolution | Tests |
|---|---|---|---|---|---|
| F01 | High | A command that failed (e.g. a mistyped id) deleted this actor's settled receipts first, so the retry of an accepted command was not recognised | fixed | `settle_commands` returns the stale receipts; `coordinated` drops them only after the new command's `build` succeeded | `test_commands`: `test_f01_failed_command_does_not_forget_an_accepted_retry` |
| F02 | High | `--description-file` was read in the locale's encoding, so a retry under ISO-8859-1 computed a different request and rewrote the accepted text (C locale: traceback) | fixed | Files and stdin are read as bytes and decoded as UTF-8 (`read_utf8`); not UTF-8 is `VALIDATION` | `test_commands`: `test_f02_locale_retry_keeps_the_accepted_description` |
| F03 | High | Uppercase schemes, passwords containing `/`, and `https://TOKEN@host` kept their secret, including in the committed `.beadroll.json` | fixed | `without_password` is case-insensitive, splits on the last `@`, and treats a user-only `http(s)` user-info as a token | `test_commands`: `test_f03_credential_urls_are_not_stored`, `test_remote_url_shapes_that_are_already_safe` |
| F04 | Medium | A process-wide umask 077 made `.beadroll.json` and `export -o` owner-only in shared checkouts | fixed | No umask; the store is private by its own modes (0700 directories, 0600 files); files written into the checkout follow the caller's umask | `test_commands`: `test_f04_checkout_files_stay_readable_in_a_shared_tree`, `test_closed_stdout_on_success_exits_zero_and_store_stays_private` |
| F05 | Medium | TAB, U+2028 and U+2029 reached the terminal raw | fixed | Added to `CONTROLS` | `test_commands`: `test_f05_human_output_escapes_tab_and_line_separators` |
| F06 | Medium | A failed command whose stderr was closed exited 120 | fixed | `report_error` silences a broken stderr and returns the command's own status | `test_commands`: `test_f06_failed_command_with_closed_stderr_keeps_its_status` |
| F07 | Medium | Non-UTF-8 argv (lone surrogates) or files produced tracebacks | fixed | `Tx.emit` checks the event encodes; files decoded strictly (F02); both `VALIDATION` | `test_commands`: `test_f07_non_utf8_input_is_a_structured_error` |
| F08 | High | A malformed dependency in an import was dropped (or crashed) after the creates were written | fixed | Dependencies are validated in the same pre-write pass as the records | `test_commands`: `test_f08_import_probes_dependencies_before_writing` |
| F09 | High | A dependency on a not-yet-existing key could not be taken back, and a re-import that dropped it did nothing; the edge returned when the target arrived and could close a cycle | fixed | `dep remove` of a dangling key emits `dep_remove {key}`; a `dep_remove` by uid is remembered as a retraction and `materialise_dangling` skips edges retracted by a writer who had seen the import; re-importing a known record retracts dangling entries it no longer lists | `test_commands`: `test_f09_dangling_dependency_can_be_retracted_before_its_target_exists`, `test_incremental_mutual_import_matches_one_shot` |
| F10 | High | An attempt file whose later line was valid JSON but not an event (`null`) confirmed the claim and lost the bundled edit | fixed | `read_attempt` validates every line as an event (envelope and payload); any failure voids the file | `test_commands`: `test_f10_non_event_json_makes_the_whole_attempt_inert` |
| F11 | High | A leftover pending twin that differed from the remote by a byte raised `ATTEMPT_FORK` although the confirmed file matched | fixed | A pending twin of an already-confirmed identical attempt is dropped, not forked | `test_commands`: `test_f11_stale_pending_twin_does_not_fork_a_matching_attempt` |
| F12 | Medium | One unreadable pending file caused a fence commit on every sync, forever | fixed | `load_pending` sweeps pending files that are not attempts (and their receipts); fencing keys on real attempts | `test_commands`: `test_f12_unreadable_pending_file_is_not_fenced_forever` |
| F13 | Medium | `meta.json`, `replica.json`, `aliases.json` were read permissively; `NaN` reached JSON output and a bad identity became a `KeyError` | fixed | All three read with `loads_strict` and shape-checked; a malformed identity is `IDENTITY` | `test_commands`: `test_f13_local_state_files_are_strict_json` |
| F14 | Medium | A clock past 2100 produced an event replay rejects, and `create` indexed it: `KeyError` | fixed | `Tx.emit` applies the replay rules before minting; `VALIDATION` with a clock message | `test_commands`: `test_f14_future_clock_create_is_a_structured_error`, `test_clock_skew_does_not_override_causality` |
| F15 | Medium | A timed-out fetch left `git-upload-pack` running | fixed | `run_git` starts git in its own session; a timed-out fetch kills the group. A timed-out *push* kills only the client and leaves the receiving side to finish or fail on its own, which is the uncertain outcome the protocol settles (killing it too would turn every lost acknowledgement into a refusal; the earlier lost-ack tests caught this) | `test_commands`: `test_f15_timeout_reaps_the_git_helper`; `test_protocol`: `test_delayed_push_after_timeout_and_absent_fetch_must_not_fork`, `test_fence_wins_against_a_real_push_still_running_after_timeout` |
| F16 | Medium | A read-only store or a full volume failed as tracebacks | fixed | `OSError` anywhere in a command is reported as a structured `IO` error; nothing half-written (the reviewer verified a full volume does not poison the next write) | `test_commands`: `test_f16_read_only_store_write_is_a_structured_error` |
| — | — | Receipts: another actor's failed claim, a `sync`, changed stdin, `--json` on the retry; safe URL shapes; `prime` vs `ready`; help outside a repo; mutual two-batch import; clock skew vs causality; JSON keeps bidi text | verified | | `test_commands`: the `test_receipt_identity_…`, `test_remote_url_shapes_…`, `test_prime_matches_ready_…`, `test_incremental_mutual_import_…`, `test_clock_skew_…`, `test_json_keeps_bidi_text` |

Measured by the review: 1,000 changing syncs grew a local bare remote to 28 MB of loose objects with auto-gc off; one `git gc --prune=now` packed it to 852 KB (GitHub does this server-side). The fast suite also passes on Python 3.12.10.

## Review 5 (`reviews/2026-10-07-5-codex.md`, ids E)

| Id | Severity | Finding | Status | Resolution | Tests |
|---|---|---|---|---|---|
| E01 | High | Human output printed raw terminal controls and embedded newlines from untrusted fields (titles, comments, names) | fixed | One output path: every human line and error message passes through `visible()`, which shows C0/C1 controls, ESC, CR and bidi controls as `\xNN`/`\uNNNN` escapes. Stored data and JSON are untouched | `test_commands`: `test_e01_human_output_escapes_terminal_controls`, `test_v01_machine_json_preserves_and_escapes_control_data` |
| E02 | High | Passwords in remote URLs were echoed by `info`/`sync`/errors and stored in `meta.json`; state files were world-readable | fixed | bead never stores or uses an embedded password: `without_password()` strips it from `--remote` (with a note) and from every resolved remote; `main` sets umask 077 so all state is 0600/0700 | `test_commands`: `test_e02_remote_credentials_are_not_disclosed_or_world_readable` |
| E03 | High | An owner's offline `delete` concurrent with a takeover left a live claim beside a tombstone conflict, which a later `release` settled to `open` | fixed | An owner's `delete` names its claim like `close`; a delete naming a superseded claim is stale and applies nothing | `test_commands`: `test_e03_takeover_fences_an_owners_concurrent_offline_delete` |
| E04 | High | `import` exited 0 and counted records whose create event replay later rejected | fixed | Every record is checked whole (envelope and payload) before anything is appended; one bad record fails the import with `VALIDATION` | `test_commands`: `test_e04_invalid_import_fails_instead_of_counting_an_inert_event` |
| E05 | Medium | `create --silent --json` printed a bare id | fixed | `--silent` returns `{"id": …}` through the normal renderer; text mode still prints just the id | `test_commands`: `test_e05_silent_create_still_honors_json_mode` |
| E06 | Medium | `NaN`/`Infinity` were accepted and re-emitted as invalid JSONL | fixed | `loads_strict` at every JSON boundary (imports, segments, attempts, config); export serialises with `allow_nan=False` | `test_commands`: `test_e06_export_never_emits_nonfinite_json_numbers` |
| E07 | Medium | A closed stdout exited 120 with a `BrokenPipeError` flush diagnostic | fixed | `main` flushes inside its own exit path and, on a broken pipe, redirects stdout to `/dev/null` and exits 0 | `test_commands`: `test_e07_closed_downstream_pipe_has_defined_quiet_exit` |
| E08 | Medium | Imported comments with null `created_at`/`author`, and dangling dependencies with a null `type`, crashed `show`, `comments` and `export` | fixed | Null dependency types normalise to `blocks` at import; human renderers show `?` for missing comment fields; export sorts with a null-safe key | `test_commands`: `test_e08_nullable_import_values_do_not_crash_render_or_export` |
| E09 | Medium | Wrong JSON types in `.beadroll.json` surfaced as tracebacks | fixed | `checked_config`: the file must hold an object; `workspace`/`ref`/`remote` strings, `where` boolean; otherwise `CONFIG` | `test_commands`: `test_e09_invalid_config_types_return_structured_config_errors` |
| E10 | Medium | A create event's `extra` could overwrite `id`, `status`, `comments` in the export | fixed | Extras are written first and never over a reserved key; the record's own fields come last | `test_replay`: `test_e10_extra_cannot_override_canonical_export_fields` |
| E11 | Medium | `--parent` matched only the first of several direct parents | fixed | Membership over every parent; `show` adds `parents` when there is more than one | `test_commands`: `test_e11_parent_filter_matches_every_direct_parent` |
| E12 | High | Under an ASCII locale, printing the ✓ after a committed `create` raised and exited 1 | fixed | The output contract is UTF-8: `main` reconfigures stdout/stderr regardless of locale | `test_commands`: `test_e12_ascii_locale_cannot_turn_a_committed_create_into_failure` |
| E13 | High | A lost-ack receipt keyed on argv ignored changed `--description-file`/stdin contents, so a follow-up edit was silently dropped | fixed | The receipt includes what the command resolved (`request`: the fields and status), so changed contents are a new request | `test_commands`: `test_e13_receipt_identity_includes_resolved_file_content` |
| — | — | `--help` works outside a repository and creates no state | verified | | `test_commands`: `test_v02_help_needs_no_repository_and_creates_no_state` |

Scale, measured by the review (synthetic single-machine histories, replay in a fresh process): 10k events 0.69 s / 45 MiB, 50k 3.4 s / 125 MiB, 200k 13.8 s / 412 MiB. The README's earlier "2 s at 200k" was from an older, smaller event shape; corrected there.

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
