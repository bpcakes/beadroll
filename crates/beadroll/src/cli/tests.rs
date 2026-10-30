//! Run lifetime and password-database lookup contracts.

use super::*;

/// A state with no issues, to be kept or freed.
fn state() -> Option<Box<dyn Held>> {
    let journal = beadroll_core::journal::Journal::new();
    let state = beadroll_core::replay::replay(journal).expect("an empty journal replays");
    Some(Box::new(state))
}

#[test]
fn the_state_a_command_answered_from_is_kept_only_by_a_process_that_ends_with_it() {
    // roll-lqjj.32 (X10): kept, never freed, when the process ends with the run
    assert!(ended_with(
        state(),
        AfterCommand::Return,
        RunLifetime::Process
    ));
    // freed before a publish, which reads a state of its own: never two at once
    assert!(!ended_with(
        state(),
        AfterCommand::Publish,
        RunLifetime::Process
    ));
    // freed by a caller that runs bead again in its process (xtask, a test), or no state
    assert!(!ended_with(
        state(),
        AfterCommand::Return,
        RunLifetime::Embedded
    ));
    assert!(!ended_with(
        state(),
        AfterCommand::Publish,
        RunLifetime::Embedded
    ));
    assert!(!ended_with(
        None,
        AfterCommand::Return,
        RunLifetime::Process
    ));
}

#[test]
fn another_users_home_comes_from_the_password_database() {
    // root has an entry everywhere, even where the user running the tests has none (a
    // container's arbitrary uid).
    let root = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(0))
        .unwrap()
        .unwrap();
    assert_eq!(
        home_of(std::ffi::OsStr::new(&root.name)),
        Some(root.dir.into_os_string())
    );
    assert_eq!(
        home_of(std::ffi::OsStr::new("no-such-user-bead-test")),
        None
    );
}
