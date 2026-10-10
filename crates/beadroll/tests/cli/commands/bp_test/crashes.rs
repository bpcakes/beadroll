//! Crashes: a claim that stopped before its push, or after it with the acknowledgement lost (the
//! crash hooks both binaries have: `BEAD_TEST_CRASH_BEFORE_PUSH` exits with status 98,
//! `BEAD_TEST_CRASH_AFTER_PUSH` with 99), and an append torn halfway through.

use std::fs::{OpenOptions, Permissions};
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::time::Duration;

use beadroll_test_support::{Json, Machine};

use super::{column, owner, segments, settle, two_machines};

/// The hook that stops a claim after the remote took its push, before bead hears back.
const CRASH_AFTER_PUSH: &str = "BEAD_TEST_CRASH_AFTER_PUSH";

/// A `git` that sleeps two seconds before every fetch, then runs the system's.
const SLOW_GIT: &str = "#!/bin/sh\n[ \"$1\" = fetch ] && sleep 2\nexec /usr/bin/git \"$@\"\n";

/// A claim the remote accepted but never acknowledged is not counted until a retry finds it
/// there; the retry claims nothing again, and every machine sees the same owner.
#[test]
fn lost_acknowledgement_is_recovered_on_retry() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    // the remote accepted; a never heard back
    let crashed = a.cmd(["claim", &x]).env(CRASH_AFTER_PUSH, "1").run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    assert!(a.show(&x).get("claim").is_none());
    let again = a.bead(["claim", &x]).unwrap()[0]["claim"].clone();
    assert_eq!(again["generation"].as_i64(), Some(1));
    let ops = column(&a.bead(["log", &x]).unwrap(), "op");
    let claims = ops.iter().filter(|op| op.as_str() == Some("claim"));
    assert_eq!(claims.count(), 1, "{ops:?}"); // found, not repeated
    b.bead(["sync"]);
    assert_eq!(b.show(&x)["claim"]["owner"], again["owner"]);
}

/// After a claim's acknowledgement is lost, the claim shows as unconfirmed and later work appends
/// after it; the next sync has nothing to pull, since the remote already had the claim, and finds
/// the claim the other machine sees, with no fork in the journal.
#[test]
fn local_work_after_a_lost_acknowledgement_does_not_fork_the_journal() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let crashed = a.cmd(["claim", &x]).env(CRASH_AFTER_PUSH, "1").run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    // not counted until the remote confirms it
    assert!(a.show(&x).get("claim").is_none());
    let claims = a.bead(["claims"]).unwrap();
    assert_eq!(
        claims["claims"][0]["state"].as_str(),
        Some("unconfirmed claim")
    );
    // appends after the claim, no gap
    a.bead(["comments", "add", &x, "work continues before anyone syncs"]);
    b.bead(["sync"]);
    let accepted = b.show(&x)["claim"].clone();
    // nothing to pull: the remote already had our claim
    assert_eq!(a.bead(["sync"]).unwrap()["pulled"].as_i64(), Some(0));
    assert_eq!(a.show(&x)["claim"], accepted);
    settle(&[&a, &b]);
    let comments = column(&b.show(&x)["comments"], "text");
    assert_eq!(comments, [Json::from("work continues before anyone syncs")]);
}

/// A claim that stopped after writing its attempt as pending, before pushing it, leaves no false
/// owner: the remote was never claimed, so another machine's claim succeeds, the stopped
/// machine's next sync finds no integrity problem, and it agrees on the new owner.
#[test]
fn a_crash_before_the_claim_is_written_leaves_no_false_owner() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    // written as pending, never pushed
    let crashed = a
        .cmd(["claim", &x])
        .env("BEAD_TEST_CRASH_BEFORE_PUSH", "1")
        .run();
    assert_eq!(crashed.returncode(), 98, "{}", crashed.stderr);
    assert!(a.show(&x).get("claim").is_none());
    let claimed = b.bead(["claim", &x]).unwrap();
    // the remote was never claimed
    assert!(owner(&claimed[0]).contains("b-thread"), "{claimed:?}");
    assert_eq!(a.bead(["sync"]).unwrap()["integrity"].as_i64(), Some(0));
    settle(&[&a, &b]);
    let issue = a.show(&x);
    assert!(owner(&issue).contains("b-thread"), "{issue:?}");
}

/// A sync that fetched before a claim from the same machine was accepted, and whose
/// acknowledgement was lost, does not void that claim: it stands everywhere.
#[test]
fn a_sync_that_fetched_before_a_lost_acknowledgement_does_not_void_it() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let slow = sandbox.root().join("slow-git");
    std::fs::create_dir(&slow).unwrap();
    std::fs::write(slow.join("git"), SLOW_GIT).unwrap();
    std::fs::set_permissions(slow.join("git"), Permissions::from_mode(0o755)).unwrap();
    let mut path = slow.into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").expect("the test's PATH"));
    let (crashed, synced) = std::thread::scope(|scope| {
        let syncing = scope.spawn(|| {
            let sync = a.cmd(["sync", "--json"]).env("PATH", &path);
            sync.thread("a-sync").run()
        });
        std::thread::sleep(Duration::from_millis(500)); // the sync is inside its first, slow fetch
        let claim = a.cmd(["claim", &x]).env(CRASH_AFTER_PUSH, "1");
        let crashed = claim.thread("a-claimer").run(); // accepted, unacknowledged
        (crashed, syncing.join().expect("the sync's thread"))
    });
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    b.bead(["sync"]);
    let issue = b.show(&x);
    // the accepted claim stands everywhere
    assert!(owner(&issue).contains("a-claimer"), "{issue:?}");
    let ops = column(&b.bead(["log", &x]).unwrap(), "op");
    assert!(!ops.contains(&Json::from("void")), "{ops:?}");
}

/// The torn tail a crash leaves halfway through an append is skipped by readers, and set aside
/// before the next write appends, kept for diagnosis.
#[test]
fn an_interrupted_append_is_repaired_before_the_next_write() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "complete record"]);
    let segment = segments(&a.store()).remove(0);
    let mut file = OpenOptions::new().append(true).open(&segment).unwrap();
    // a crash mid-write
    let torn = br#"{"v": 1, "id": "01a0c450-0000-7000-8000-00000000"#;
    file.write_all(torn).unwrap();
    drop(file);
    // readers skip the torn tail
    assert_eq!(a.bead(["list"]).unwrap()["total"].as_i64(), Some(1));
    a.bead(["create", "written after the interruption"]);
    // and the next write did not poison the file
    assert_eq!(a.bead(["list"]).unwrap()["total"].as_i64(), Some(2));
    let folder = std::fs::read_dir(segment.parent().unwrap()).unwrap();
    let names: Vec<String> = folder
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    // the fragment is kept for diagnosis
    assert!(
        names.iter().any(|name| name.contains(".torn-")),
        "{names:?}"
    );
}
