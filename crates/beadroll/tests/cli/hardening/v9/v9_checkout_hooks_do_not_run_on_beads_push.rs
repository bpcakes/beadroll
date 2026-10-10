//! The checkout's own hooks are the user's pushes' business, never bead's.

use std::os::unix::fs::PermissionsExt as _;

use beadroll_test_support::{Machine, git};

use super::scratch_git_config;

/// Verified (V9): per-repository hooks do not leak into bead's push. A refusing
/// .git/hooks/pre-push in the checkout and a refusing husky hook (per-repository
/// core.hooksPath=.husky) never run, and sync and claim succeed. The review measured 0 hook runs
/// out of the 2 refusing per-repository hooks, on both sync and claim.
#[test]
fn checkout_and_husky_pre_push_hooks_never_run() {
    let sandbox = crate::sandbox();
    let marker = sandbox.root().join("ran");
    let mut a = Machine::new(&sandbox, "a");
    a.extra.extend(scratch_git_config(&sandbox, ""));
    let hooks = [
        a.checkout().join(".git").join("hooks").join("pre-push"),
        a.checkout().join(".husky").join("pre-push"),
    ];
    for hook in hooks {
        std::fs::create_dir_all(hook.parent().expect("a hooks folder")).unwrap();
        let script = format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display());
        std::fs::write(&hook, script).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(["config", "core.hooksPath", ".husky"], Some(a.checkout()));
    a.bead(["init"]);
    let created = a.bead(["create", "one"]).expect("the created issue");
    let issue = created["id"].as_str().expect("an id").to_owned();
    a.bead(["sync"]);
    assert!(
        !marker.exists(),
        "a checkout pre-push hook ran on bead sync"
    );
    a.bead(["claim", &issue]);
    assert!(
        !marker.exists(),
        "a checkout pre-push hook ran on bead claim"
    );
    assert_eq!(a.show(&issue)["status"].as_str(), Some("in_progress"));
}
