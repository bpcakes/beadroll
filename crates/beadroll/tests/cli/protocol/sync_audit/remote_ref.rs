//! The remote's journal ref when it is not what bead published: rolled back, deleted, pointed at a
//! commit that is no journal, or another workspace's.

use beadroll_test_support::{Machine, code};

use super::{remote_git, setup};

/// A remote journal rolled back to an older commit, and later deleted, is restored from the copy a
/// machine still holds: its next sync publishes everything again, and a new machine reads the
/// whole tracker each time.
#[test]
fn remote_rollback_and_deleted_ref_restore_from_surviving_copy() {
    let sandbox = crate::sandbox();
    let (a, _) = setup(&sandbox);
    a.bead(["sync"]);
    let old_tip = remote_git(&sandbox, &["rev-parse", "refs/beads/journal"]);
    let old_tip = old_tip.trim();
    a.bead(["create", "survives rollback"]);
    a.bead(["sync"]);
    let expected = a.export();
    remote_git(&sandbox, &["update-ref", "refs/beads/journal", old_tip]);
    a.bead(["sync"]);
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    assert_eq!(b.export(), expected);
    remote_git(&sandbox, &["update-ref", "-d", "refs/beads/journal"]);
    a.bead(["sync"]);
    let c = Machine::new(&sandbox, "c");
    c.bead(["sync"]);
    assert_eq!(c.export(), expected);
}

/// A journal ref that points at a commit without a manifest, the code's `main`, is refused with
/// MANIFEST_MISSING; once the ref is gone and the journal published again, a sync under another
/// workspace's name is refused with WORKSPACE_MISMATCH.
#[test]
fn manifest_missing_and_foreign_workspace_are_refused() {
    let sandbox = crate::sandbox();
    let (a, _) = setup(&sandbox);
    a.bead(["sync"]);
    let code_tip = remote_git(&sandbox, &["rev-parse", "main"]);
    remote_git(
        &sandbox,
        &["update-ref", "refs/beads/journal", code_tip.trim()],
    );
    assert_eq!(code(&a.run(["sync", "--json"])), "MANIFEST_MISSING");
    remote_git(&sandbox, &["update-ref", "-d", "refs/beads/journal"]);
    a.bead(["sync"]);
    let elsewhere = a.cmd(["sync", "--json"]);
    let elsewhere = elsewhere.env("BEAD_WORKSPACE", "a-different-workspace");
    assert_eq!(code(&elsewhere.run()), "WORKSPACE_MISMATCH");
}
