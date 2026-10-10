//! `ActualTransportReview`: a release the remote accepted while bead heard nothing back (its push
//! still running when bead stopped waiting, or its acknowledgement lost) is found accepted and
//! reported as a success, never built again (the third independent review, 2026-10-06).

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use beadroll_store::glob_ending;
use beadroll_test_support::{Machine, Sandbox};

/// Machine `NAME-a`'s tracker with one issue, "release target", synced to machine `NAME-b`, and
/// then claimed by machine a: both machines and the issue's key.
fn claimed_target(sandbox: &Sandbox, name: &str) -> (Machine, Machine, String) {
    let a = Machine::new(sandbox, &format!("{name}-a"));
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "release target"]).unwrap();
    let key = created["id"]
        .as_str()
        .expect("the new issue's id")
        .to_owned();
    a.bead(["sync"]);
    let b = Machine::new(sandbox, &format!("{name}-b"));
    b.bead(["sync"]);
    a.bead(["claim", &key]);
    (a, b, key)
}

/// The attempt files `machine` has pending.
fn pending(machine: &Machine) -> Vec<PathBuf> {
    glob_ending(&machine.store().join("pending"), ".json")
}

/// `path` quoted for the shell.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// Actual receive-pack timeout: a remotely accepted release must not be rebuilt.
#[test]
fn timed_out_accepted_release_returns_success() {
    let sandbox = crate::sandbox();
    let (a, b, key) = claimed_target(&sandbox, "timeout");

    let first = sandbox.root().join("first-release-push");
    let finished = sandbox.root().join("first-release-finished");
    let hook = sandbox.origin().join("hooks").join("pre-receive");
    // only the first push is slow, and it outlasts bead's patience
    let script = format!(
        "#!/bin/sh\nif mkdir {} 2>/dev/null; then\n  sleep 0.15\n  touch {}\nfi\nexit 0\n",
        quoted(&first),
        quoted(&finished)
    );
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    let result = a
        .cmd(["release", &key, "--json"])
        .env("BEAD_NET_TIMEOUT", "0.1")
        .run();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !finished.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(finished.exists(), "actual receive-pack did not finish");
    b.bead(["sync"]);
    assert!(a.show(&key).get("claim").is_none());
    assert!(b.show(&key).get("claim").is_none());
    assert_eq!(pending(&a), Vec::<PathBuf>::new());
    assert_eq!(
        result.returncode(),
        0,
        "remote accepted the release, but recovery rebuilt it: {}",
        result.stderr
    );
}

/// A release the remote accepted whose acknowledgement was lost (`BEAD_TEST_CRASH_AFTER_PUSH`) is
/// recognised when the command runs again: the retry succeeds, no machine shows the claim, and
/// nothing is left pending.
#[test]
fn crash_after_accepted_release_can_be_retried_idempotently() {
    let sandbox = crate::sandbox();
    let (a, b, key) = claimed_target(&sandbox, "crash");

    let crashed = a
        .cmd(["release", &key, "--json"])
        .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
        .run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    let retry = a.run(["release", &key, "--json"]);
    b.bead(["sync"]);
    assert!(a.show(&key).get("claim").is_none());
    assert!(b.show(&key).get("claim").is_none());
    assert_eq!(pending(&a), Vec::<PathBuf>::new());
    assert_eq!(
        retry.returncode(),
        0,
        "accepted release was confirmed, but a command retry reports: {}",
        retry.stderr
    );
}
