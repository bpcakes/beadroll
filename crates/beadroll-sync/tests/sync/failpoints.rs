//! Direct checks of failpoint contracts used by protocol tests
//! (`crates/beadroll-store/src/failpoint.rs`).

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use beadroll_store::Endpoint;
use beadroll_sync::PublishMode;
use beadroll_sync::{Error, Link, PushOutcome, locked};
use beadroll_test_support::{Machine, git};

use crate::support::{quoted, sandbox, workspace};

/// The git on `PATH`.
fn real_git() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|git| git.is_file())
        .expect("git on PATH")
}

/// `fail-reply` makes a push the remote took a lost acknowledgement whatever git's exit status
/// said: here git pushes and still exits 1, and the status line that names the ref counts as
/// acceptance on its own, so without the failpoint the publish is accepted, and with it the answer
/// is lost although the remote holds the commit.
#[test]
fn a_lost_reply_drops_acceptance_whatever_the_exit_status() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["sync"]);
    let wrapper = sandbox.root().join("git-wrapper");
    std::fs::create_dir(&wrapper).unwrap();
    let real = quoted(&real_git());
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = push ]; then\n  {real} \"$@\" || exit $?\n  exit 1\nfi\nexec {real} \"$@\"\n"
    );
    std::fs::write(wrapper.join("git"), script).unwrap();
    let mode = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(wrapper.join("git"), mode).unwrap();
    let path = format!(
        "{}:{}",
        wrapper.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let origin = sandbox.origin().as_os_str();
    let tip = || {
        let args = [
            "--git-dir".as_ref(),
            origin,
            "rev-parse".as_ref(),
            "refs/beads/journal".as_ref(),
        ];
        git(args, None)
    };
    for (failpoints, answer) in [
        ("", PushOutcome::Ok),
        ("push.reply=fail-reply", PushOutcome::Uncertain),
    ] {
        let ws = workspace(
            &a,
            sandbox.origin(),
            &[("PATH", &path), ("BEAD_FAILPOINTS", failpoints)],
        );
        let before = tip();
        let outcome = locked(&ws, || -> Result<PushOutcome, Error> {
            let mut link = Link::new(&ws, &Endpoint::at(origin))?;
            let fetched = link.fetch()?;
            link.merge(fetched.as_deref())?;
            link.publish(fetched.as_deref(), "fence", &[], PublishMode::Fence)
        })
        .unwrap();
        assert_eq!(outcome, answer, "{failpoints:?}");
        assert_ne!(tip(), before, "the remote took the push: {failpoints:?}");
        let fired = u64::from(!failpoints.is_empty());
        assert_eq!(ws.settings.failpoints.hits("push.reply"), fired);
    }
}
