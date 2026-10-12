//! `V1_StalledReceiveTimeouts`: stalled receive sides do not hold bead past BEAD_NET_TIMEOUT; their
//! outcomes settle later.

use std::path::Path;
use std::time::{Duration, Instant};

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, Sandbox, code, parse_json};

use super::{executable, id, member, quoted};

/// Machine a, its tracker initialised with the prefix `ex`, the issue `one`
/// created and synced. Returns the machine and the issue's key.
fn set_up(sandbox: &Sandbox) -> (Machine, String) {
    let a = Machine::new(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = id(a.bead(["create", "one"]));
    a.bead(["sync"]);
    (a, key)
}

/// Waits up to 15 seconds for `path` to exist, looking every tenth of a second.
fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Verified (V1): a receive side that floods stderr and then stalls does not hold bead past
/// BEAD_NET_TIMEOUT (no hang, no pipe deadlock); the receive side finishes on its own and the next
/// sync settles it. The review measured, at BEAD_NET_TIMEOUT=2, REMOTE_UNREACHABLE after 2.33 s
/// (pre-receive sleeping 6 s), 2.32 s (200 KiB on stderr, then 6 s), 2.34 s and 2.34 s (a
/// backgrounded child holding stderr, pre- and post-receive); leftovers were only git-receive-pack
/// and the hook, reparented, no zombies, all gone after 7 s. Its TestCase: at 1.5 s the stalled
/// sync returned in under 3.5 s, and the next sync left no pending attempts.
#[test]
fn stalled_receive_with_a_stderr_flood_returns_within_the_timeout() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    let done = sandbox.root().join("hook-done");
    let hook = sandbox.origin().join("hooks").join("pre-receive");
    let script = format!(
        "#!/bin/sh\ncat >/dev/null\nhead -c 204800 /dev/zero | tr '\\0' x >&2\nsleep 4\ntouch \
         {}\nexit 0\n",
        quoted(&done)
    );
    executable(&hook, &script);
    // the sync below meets the stall
    a.bead(["comments", "add", &key, "stalled", "--offline"]);
    let started = Instant::now();
    let stalled = a
        .cmd(["sync", "--json"])
        .env("BEAD_NET_TIMEOUT", "1.5")
        .run();
    let took = started.elapsed();
    assert!(took < Duration::from_secs_f64(1.5 + 2.0), "{took:?}");
    assert_eq!(code(&stalled), "REMOTE_UNREACHABLE");
    wait_for(&done);
    assert!(done.exists(), "the receive side never finished");
    std::fs::remove_file(&hook).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let settled = a.run(["sync", "--json"]);
    assert_eq!(settled.returncode(), 0, "{}", settled.stderr);
    let info = a.bead(["info"]).expect("bead info prints a record");
    assert_eq!(member(&info, "pending_ownership"), &Json::Array(Vec::new()));
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    let shown = write(&b.show(&key), Format::ASCII).unwrap();
    assert!(shown.contains("stalled"), "{shown}");
}

/// Verified (V1): a claim whose acknowledgement is held by a slow post-receive (the ref is already
/// updated) is recovered rather than reported lost. The review measured, with a 4 s post-receive
/// at BEAD_NET_TIMEOUT=1.5: claim rc 0, claim generation 1, no pending ownership left.
#[test]
fn claim_whose_ack_is_held_by_a_slow_post_receive_is_recovered() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    let gate = sandbox.root().join("slow-once");
    let done = sandbox.root().join("slow-done");
    let hook = sandbox.origin().join("hooks").join("post-receive");
    let script = format!(
        "#!/bin/sh\ncat >/dev/null\nif mkdir {} 2>/dev/null; then sleep 4; touch {}; fi\nexit 0\n",
        quoted(&gate),
        quoted(&done)
    );
    executable(&hook, &script);
    let claimed = a
        .cmd(["claim", &key, "--json"])
        .env("BEAD_NET_TIMEOUT", "1.5")
        .run();
    assert_eq!(claimed.returncode(), 0, "{}", claimed.stderr);
    assert!(gate.exists(), "the slow post-receive never ran");
    let printed = parse_json(&claimed.stdout).unwrap();
    let claim = member(&printed[0], "claim");
    assert_eq!(member(claim, "generation").as_i64(), Some(1), "{claim:?}");
    let info = a.bead(["info"]).expect("bead info prints a record");
    assert_eq!(member(&info, "pending_ownership"), &Json::Array(Vec::new()));
    // let the receive side finish before the sandbox goes
    wait_for(&done);
}
