//! Black-box hardening tests, a module for each finding the port of Python's suite brought with it,
//! named for what it checks; its doc names the finding (`docs/FINDINGS.md`). The `v*` modules still
//! carry only their finding's id: history, not a pattern, since a new test goes in a module named
//! for its subject (`AGENTS.md`). Fault injection uses failpoints on this build's binary
//! (`crate::rust_only`); `beadroll_test_support::Gate` coordinates a test with a paused process.

/// How a test machine first opens its tracker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackerSetup {
    /// The tracker another machine published, fetched with `bead sync`.
    Sync,
    /// A tracker started here with `bead init --prefix ex`.
    Init,
}

mod arabic_letter_mark_escaped;
mod at_sign_after_the_authority;
mod changed_origin_url_is_followed;
mod client_hooks_on_private_pushes;
mod closed_stdin_is_a_structured_error;
mod concurrent_processes_share_no_fetch_destination;
mod damaged_meta_never_moves_coordination;
mod damaged_remote_is_not_an_unreachable_one;
mod deep_json_is_bounded_and_scoped;
mod definite_refusal_is_neither_a_race_nor_no_answer;
mod dubious_ownership;
mod duplicate_ids_in_one_import_fail_whole;
mod export_with_unicode_line_separators_imports_again;
mod failed_atomic_write_leaves_nothing_behind;
mod failure_after_acceptance_is_not_nothing_half_done;
mod finished_push_is_not_an_unanswered_one;
mod fixture_environment;
pub(crate) mod help_is_plain_text;
mod https_token_in_the_user_position;
mod hung_up_terminal_and_closed_descriptors;
mod import_comments_of_the_wrong_shape_are_validation;
mod integer_fields_stay_beads_readable;
mod json_says_when_the_end_of_a_claim_was_not_published;
mod key_retraction_survives_a_concurrent_target;
mod local_state_of_the_wrong_shape_is_an_error;
mod lone_surrogate_in_a_journal_is_scoped;
mod lost_race_keeps_the_in_flight_receipt;
mod machines_name_keeps_aliases_it_could_not_read;
mod materialised_dependency_exports_its_meta;
mod merged_check_sees_origin_main;
mod new_key_never_takes_an_awaited_key;
mod non_utf8_git_and_ps_output_is_not_a_traceback;
mod object_format;
mod only_the_lock_holder_sweeps_pending;
mod origin_is_where_git_pushes;
mod origins_git_reads_as_remotes;
mod password_in_a_transport_url;
mod receipt_survives_later_local_writes;
mod receipt_that_is_not_an_object_blocks_nothing;
mod recognised_retry_reports_a_claim_that_moved_on;
mod remote_resolved_against_the_cwd;
mod renamed_record_exports_under_its_own_id;
mod renamed_repository_keeps_its_tracker;
mod retraction_survives_a_twin_of_the_imported_key;
mod retried_end_of_claim_is_published;
mod self_dependent_import_fails_whole;
mod server_hook_text_does_not_decide_the_push_outcome;
mod settle_keeps_line_level_integrity_reports;
mod stale_receipts_dropped_before_the_command_goes_through;
mod stale_twin_cleanup_keeps_the_receipt;
mod store_is_private_by_its_own_modes;
mod store_never_lands_in_the_checkout;
mod superseded_owner_close_says_so;
mod tracked_config_cannot_adopt_another_repositorys_store;
mod tracked_config_is_utf8_whatever_the_locale;
mod tracked_config_read_from_working_tree_only;
mod tracked_init_keeps_hand_set_config;
mod tracked_remote_is_never_a_git_option;
mod translated_git;
mod transport_settings;
mod unfinished_sync_git_is_rebuilt;
mod unreadable_identity_is_io;
mod unreadable_store_is_io;
mod unresolved_imported_dependency_is_shown;
mod unusable_tracked_config_is_an_error;
mod v1;
mod v2;
mod v3;
mod v4;
mod v5;
mod v6;
mod v7;
mod v8;
mod v9;
mod writes_on_a_broken_own_chain;
