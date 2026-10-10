//! Lost acknowledgements: a claim the remote took whose answer never reached bead, because bead
//! was killed, crashed (the crash hook both binaries have, `BEAD_TEST_CRASH_AFTER_PUSH`, which
//! exits with status 99) or stopped waiting.

use std::time::Duration;

use beadroll_test_support::{Json, code, parse_json};

use super::{Worker, git_wrapper, setup, two, wait_marker};

/// The claim on `issue`, `issue["claim"]`: an issue without one fails the test, as the Python
/// test's lookup raises.
fn claim(issue: &Json) -> Json {
    let claim = issue.get("claim").cloned();
    claim.unwrap_or_else(|| panic!("no claim in {issue:?}"))
}

/// The operation of every row of `bead log`, `[r["op"] for r in rows]`: a row without one fails
/// the test, as the Python test's lookup raises.
fn ops(rows: &Json) -> Vec<&str> {
    let rows = rows
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {rows:?}"));
    rows.iter()
        .map(|row| {
            row["op"]
                .as_str()
                .unwrap_or_else(|| panic!("no op in {row:?}"))
        })
        .collect()
}

/// A claim killed after the remote took its push, before bead heard back, is not counted; the next
/// claim finds the attempt the remote accepted and confirms it, generation 1 with one claim in the
/// log, and the other machine sees the same claim.
#[test]
fn claim_lost_ack_after_real_push_kill_is_recovered() {
    let sandbox = crate::sandbox();
    let (a, key) = setup(&sandbox);
    let b = two(&sandbox, &a);
    let marker = sandbox.root().join("pushed");
    let path = git_wrapper(&sandbox, "push", &marker, true);
    let mut worker = Worker::start(a.cmd(["claim", &key, "--json"]).env("PATH", path), true);
    wait_marker(&marker, &mut worker);
    worker.kill().expect("the claim's process group");
    worker.communicate(Some(Duration::from_secs(5)));
    assert!(a.show(&key).get("claim").is_none());
    let recovered = claim(&a.bead(["claim", &key]).unwrap()[0]);
    assert_eq!(recovered["generation"].as_i64(), Some(1));
    let log = a.bead(["log", &key]).unwrap();
    let claims = ops(&log).into_iter().filter(|op| *op == "claim").count();
    assert_eq!(claims, 1);
    b.bead(["sync"]);
    assert_eq!(claim(&b.show(&key)), recovered);
}

/// A local write after a claim whose acknowledgement was lost does not fork the machine's own
/// journal: the claim is not counted, the comment is written, and the next sync succeeds, with the
/// claim the other machine sees and the comment both kept.
#[test]
fn bug_local_write_after_lost_ack_must_not_fork_accepted_claim() {
    let sandbox = crate::sandbox();
    let (a, key) = setup(&sandbox);
    let b = two(&sandbox, &a);
    let crashed = a.cmd(["claim", &key, "--json"]);
    let result = crashed.env("BEAD_TEST_CRASH_AFTER_PUSH", "1").run();
    assert_eq!(result.returncode(), 99);
    assert!(a.show(&key).get("claim").is_none());
    let text = "local work after lost acknowledgement";
    let local = a.run(["comments", "add", &key, text, "--json"]);
    assert_eq!(local.returncode(), 0, "{}", local.stderr);
    b.bead(["sync"]);
    let accepted = claim(&b.show(&key));
    let recovered = a.run(["sync", "--json"]);
    assert_eq!(
        recovered.returncode(),
        0,
        "an accepted online claim and acknowledged local comment now occupy forked own journals: {}",
        recovered.stderr
    );
    assert_eq!(claim(&a.show(&key)), accepted);
    assert_eq!(a.show(&key)["comments"][0]["text"].as_str(), Some(text));
}

/// A claim whose push the remote applied but that gave no answer within the network timeout (0.4
/// seconds; the push is held 20) is recovered: either a later round of the same command finds the
/// attempt on the remote and confirms it, or the command fails as unreachable or unresolved,
/// counting nothing, and the next claim confirms it. Generation 1 either way, and the other machine
/// sees the same claim.
#[test]
fn claim_timeout_after_real_applied_push_is_recovered() {
    let sandbox = crate::sandbox();
    let (a, key) = setup(&sandbox);
    let b = two(&sandbox, &a);
    let marker = sandbox.root().join("pushed");
    let path = git_wrapper(&sandbox, "push", &marker, true);
    let claiming = a.cmd(["claim", &key, "--json"]).env("PATH", path);
    let result = claiming.env("BEAD_NET_TIMEOUT", "0.4").run();
    assert!(marker.exists());
    let recovered = if result.returncode() == 0 {
        // The attempt stayed pending after the unanswered push; the next round of the same
        // command fetched the head, found the attempt there and confirmed it.
        claim(&parse_json(&result.stdout).unwrap()[0])
    } else {
        let code = code(&result);
        assert!(
            ["REMOTE_UNREACHABLE", "UNRESOLVED"].contains(&code.as_str()),
            "{code}"
        );
        assert!(a.show(&key).get("claim").is_none());
        claim(&a.bead(["claim", &key]).unwrap()[0])
    };
    assert_eq!(recovered["generation"].as_i64(), Some(1));
    assert_eq!(claim(&a.show(&key)), recovered);
    b.bead(["sync"]);
    assert_eq!(claim(&b.show(&key)), recovered);
}
