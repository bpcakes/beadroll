//! AS08: exercise the test-local Git writers, not just the shared command builder.

#[test]
fn fixture_git_writes_ignore_inherited_repository_and_object_routing() {
    beadroll_test_support::assert_git_isolation(&[
        "commands::bp_test::checkouts::a_commit_made_with_the_clock_ahead_is_merged_once_main_has_it",
        "hardening::g34::g34_non_utf8_git_and_ps_output_is_not_a_traceback::every_command_with_an_origin_url_that_is_not_utf8",
        "hardening::g34::g34_non_utf8_git_and_ps_output_is_not_a_traceback::create_on_a_branch_whose_name_is_not_utf8",
        "hardening::g34::g34_non_utf8_git_and_ps_output_is_not_a_traceback::show_closed_issue_when_origin_head_names_a_latin1_branch",
        "hardening::v3::v3_foreign_folder_default_quote_path::non_utf8_ref_folder_does_not_crash_sync_under_default_quotepath",
    ]);
}
