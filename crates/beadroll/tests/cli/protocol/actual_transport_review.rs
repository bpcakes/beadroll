//! `ActualTransportReview`: a release the remote accepted while bead heard nothing back (its push
//! still running when bead stopped waiting, or its acknowledgement lost) is found accepted and
//! reported as a success, never built again; one whose push had not landed when bead's next round
//! began is fenced by that round and built again, once (the third independent review,
//! 2026-10-06).

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use beadroll_store::glob_ending;
use beadroll_test_support::{Machine, Run, Sandbox, git};

use crate::hardening::gate::{Gate, beside};

/// Machine `NAME-a`'s tracker with one issue, "release target", synced to machine `NAME-b`, and
/// then claimed by machine a: both machines (made by `make`) and the issue's key.
fn claimed_target(
    sandbox: &Sandbox,
    name: &str,
    make: fn(&Sandbox, &str) -> Machine,
) -> (Machine, Machine, String) {
    let a = make(sandbox, &format!("{name}-a"));
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "release target"]).unwrap();
    let key = created["id"]
        .as_str()
        .expect("the new issue's id")
        .to_owned();
    a.bead(["sync"]);
    let b = make(sandbox, &format!("{name}-b"));
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

/// The kind of each commit on the remote's journal, oldest first ("claim", "fence", "sync").
fn remote_kinds(sandbox: &Sandbox) -> Vec<String> {
    let args = ["log", "--reverse", "--format=%s", "refs/beads/journal"];
    let subjects = git(args, Some(sandbox.origin()));
    let words = subjects.split_whitespace();
    let kinds = words.filter(|word| ["claim", "fence", "sync"].contains(word));
    kinds.map(str::to_owned).collect()
}

/// Has the remote's pre-receive hook hold the first push it sees until the file it returns exists
/// (a minute at most), and let every later push through at once.
fn hold_first_push(sandbox: &Sandbox, name: &str) -> PathBuf {
    let first = sandbox.root().join(format!("{name}-first-push"));
    let go = sandbox.root().join(format!("{name}-go"));
    let hook = sandbox.origin().join("hooks").join("pre-receive");
    let script = format!(
        "#!/bin/sh\nif mkdir {} 2>/dev/null; then\n  i=0\n  while [ ! -e {} ] && [ $i -lt 1200 ]; \
         do\n    sleep 0.05\n    i=$((i + 1))\n  done\nfi\nexit 0\n",
        quoted(&first),
        quoted(&go)
    );
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    go
}

/// Machine `a`'s release of `key` with a second for each fetch and push, every wait between rounds
/// stopped at `between` until it opens.
fn release(a: &Machine, key: &str, between: &Gate) -> Run {
    let failpoints = format!("backoff@always={}", between.action());
    let release = a.cmd(["release", key, "--json"]);
    let release = release.env("BEAD_NET_TIMEOUT", "1");
    release.env("BEAD_FAILPOINTS", &failpoints).run()
}

/// Actual receive-pack timeout, the push landing while bead waits: a remotely accepted release must
/// not be rebuilt. The release's push is held in the remote's pre-receive hook past bead's
/// patience; once bead has given up on it and waits between rounds (a gate on `backoff`), the push
/// is let go and lands, and bead's next round finds the release accepted: one claim more on the
/// remote and no fence. The rounds' order is the gate's and the hook's, and the command's other
/// fetches and pushes have a second each, far more than they take. With a 0.1 s timeout on every
/// one of them and a random wait between rounds deciding the order, a fetch or the fence sometimes
/// missed its tenth of a second on a macOS runner (roll-dih8.28); the fenced order is
/// `timed_out_release_fenced_by_its_own_next_round_is_built_again`.
#[test]
fn timed_out_accepted_release_returns_success() {
    let sandbox = crate::sandbox();
    let (a, b, key) = claimed_target(&sandbox, "timeout", crate::rust_only);
    let go = hold_first_push(&sandbox, "timeout");
    let before = remote_kinds(&sandbox);
    let between = Gate::new(&sandbox, "timeout-between-rounds");
    let (result, landed) = beside(
        || release(&a, &key, &between),
        |finished| {
            // bead gave up on the push and waits between rounds: the push lands now
            between.meet(finished, || {
                std::fs::write(&go, b"").unwrap();
                let deadline = Instant::now() + Duration::from_secs(60);
                while remote_kinds(&sandbox).len() == before.len() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                remote_kinds(&sandbox).len() > before.len()
            })
        },
    );
    // the hook never waits out its minute, whatever came of the gate
    std::fs::write(&go, b"").unwrap();
    assert_eq!(
        landed,
        Some(true),
        "bead did not wait between rounds, or the held push never landed: {}",
        result.stderr
    );
    assert_eq!(
        result.returncode(),
        0,
        "remote accepted the release, but recovery rebuilt it: {}",
        result.stderr
    );
    assert_eq!(remote_kinds(&sandbox)[before.len()..], ["claim"]);
    b.bead(["sync"]);
    assert!(a.show(&key).get("claim").is_none());
    assert!(b.show(&key).get("claim").is_none());
    assert_eq!(pending(&a), Vec::<PathBuf>::new());
}

/// Actual receive-pack timeout, the push still held when bead's next round begins: that round
/// fences it and builds the release again, once. The gate opens with the push still in the
/// remote's hook; the round's fence lands, the release is built and published again, and the held
/// push, let go once bead is done, can no longer land on the head that moved: a fence and one claim
/// more on the remote, and the claim gone on both machines.
#[test]
fn timed_out_release_fenced_by_its_own_next_round_is_built_again() {
    let sandbox = crate::sandbox();
    let (a, b, key) = claimed_target(&sandbox, "fenced", crate::rust_only);
    let go = hold_first_push(&sandbox, "fenced");
    let before = remote_kinds(&sandbox);
    let between = Gate::new(&sandbox, "fenced-between-rounds");
    let (result, met) = beside(
        || release(&a, &key, &between),
        // the gate opens with the push still held
        |finished| between.meet(finished, || ()),
    );
    // bead is done: the held push goes on, on a head that has moved
    std::fs::write(&go, b"").unwrap();
    assert_eq!(
        met,
        Some(()),
        "bead did not wait between rounds: {}",
        result.stderr
    );
    assert_eq!(result.returncode(), 0, "{}", result.stderr);
    assert_eq!(remote_kinds(&sandbox)[before.len()..], ["fence", "claim"]);
    b.bead(["sync"]);
    assert!(a.show(&key).get("claim").is_none());
    assert!(b.show(&key).get("claim").is_none());
    assert_eq!(pending(&a), Vec::<PathBuf>::new());
}

/// A release the remote accepted whose acknowledgement was lost (`BEAD_TEST_CRASH_AFTER_PUSH`) is
/// recognised when the command runs again: the retry succeeds, no machine shows the claim, and
/// nothing is left pending.
#[test]
fn crash_after_accepted_release_can_be_retried_idempotently() {
    let sandbox = crate::sandbox();
    let (a, b, key) = claimed_target(&sandbox, "crash", Machine::new);

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
