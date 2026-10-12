//! The remote's journal ref when it is not what bead published: rolled back, deleted, pointed at a
//! commit that is no journal, another workspace's, or moved on by someone else before every push
//! bead makes.

use std::ffi::OsString;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use beadroll_sync::ROUNDS;
use beadroll_test_support::{Machine, Sandbox, code};

use super::{quoted, real_git, remote_git, setup, setup_with, two_with};

/// A `PATH` whose first `git` moves the remote's journal on right before every push: a commit on
/// the tip of the sandbox remote's `refs/beads/journal` with the same tree, written there directly
/// under the identity `audit`, and a line in `counter`; then the real push runs, which the remote
/// rejects as no fast-forward. Every other git command runs unchanged. Python's wrapper is a Python
/// script; this one is a shell script that does the same.
fn hostile_git(sandbox: &Sandbox, counter: &Path) -> OsString {
    let folder = sandbox.root().join("hostile-git");
    std::fs::create_dir(&folder).unwrap();
    let script = r#"#!/bin/sh
if [ "$1" = push ]; then
  hostile() {
    GIT_AUTHOR_NAME=audit GIT_AUTHOR_EMAIL=audit@example.invalid \
    GIT_COMMITTER_NAME=audit GIT_COMMITTER_EMAIL=audit@example.invalid \
    @git@ --git-dir @origin@ "$@"
  }
  tip=$(hostile rev-parse refs/beads/journal) || exit 1
  tree=$(hostile rev-parse "$tip^{tree}") || exit 1
  moved=$(hostile commit-tree "$tree" -p "$tip" -m 'hostile advancement') || exit 1
  hostile update-ref refs/beads/journal "$moved" || exit 1
  echo moved >> @counter@
fi
exec @git@ "$@"
"#
    .replace("@git@", &quoted(&real_git()))
    .replace("@origin@", &quoted(sandbox.origin()))
    .replace("@counter@", &quoted(counter));
    let wrapper = folder.join("git");
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, Permissions::from_mode(0o755)).unwrap();
    let mut path = folder.into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").expect("the test's PATH"));
    path
}

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

/// The remote moves on before every push: a claim, then a sync, each give up after exactly
/// [`ROUNDS`] rounds (twelve, as Python's, until roll-lqjj.7) with SYNC_CONFLICT and leave the
/// tracker as it was, the edit made offline kept and no claim, nothing partial. Once the remote
/// holds still, a sync publishes everything and nothing is left waiting.
///
/// Python's driver replaces `time.sleep` for the whole run so that no round waits for the next;
/// here the Rust `bead` (`rust_only`) skips every wait between rounds, `backoff@always=skip`, and
/// the pushes the hostile `git` counts, [`ROUNDS`] a command, show that every round ran.
#[test]
fn retry_exhaustion_retains_local_edits_and_no_partial_claim() {
    let sandbox = crate::sandbox();
    let (a, key) = setup_with(&sandbox, crate::rust_only);
    two_with(&sandbox, &a, crate::rust_only);
    a.bead(["create", "unpublished edit", "--offline"]);
    // Move the actual local remote forward immediately before each attempted push.
    // The transport and fast-forward rejection remain real; only backoff is suppressed.
    let counter = sandbox.root().join("rejections");
    let path = hostile_git(&sandbox, &counter);
    let before = a.export();
    let commands: [&[&str]; 2] = [&["claim", &key, "--json"], &["sync", "--json"]];
    for (index, args) in (1..).zip(commands) {
        let command = args[0];
        let result = a
            .cmd(args)
            .env("PATH", &path)
            .env("BEAD_FAILPOINTS", "backoff@always=skip")
            .run();
        assert_eq!(
            code(&result),
            "SYNC_CONFLICT",
            "{command}: {}",
            result.stderr
        );
        let moved = std::fs::read_to_string(&counter).unwrap();
        let rounds = usize::try_from(ROUNDS).unwrap();
        assert_eq!(moved.lines().count(), rounds * index, "{command}");
        assert_eq!(a.export(), before, "{command}");
        let issue = a.show(&key);
        assert!(issue.get("claim").is_none(), "{command}: {issue:?}");
    }
    a.bead(["sync"]);
    assert_eq!(
        a.bead(["info"]).unwrap()["waiting_events"].as_i64(),
        Some(0)
    );
}
