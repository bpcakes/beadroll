//! Claims: exclusive online, provisional offline, lasting until release, close or takeover, and
//! leaving no trace when refused.

use beadroll_test_support::{Json, Machine, code};

use super::{column, length, owner, pool_map, settle, sorted_codes, two_machines};

/// `(id, state, generation)` of every claim `bead claims` lists on `machine`.
fn claim_rows(machine: &Machine) -> Vec<(Json, Json, Json)> {
    let listed = machine.bead(["claims"]).unwrap();
    let claims = listed["claims"].as_array().unwrap();
    let row = |c: &Json| (c["id"].clone(), c["state"].clone(), c["generation"].clone());
    claims.iter().map(row).collect()
}

/// An online claim is exclusive across machines: another machine's claim is refused without a
/// sync by hand, and that machine then sees the owner; of two claims made at the same instant
/// through `update --status in_progress`, one wins, and the refused one leaves no trace in the
/// journal.
#[test]
fn online_claim_is_exclusive_across_machines() {
    let sandbox = crate::sandbox();
    let (a, b, x, y) = two_machines(&sandbox);
    let first = a.bead(["claim", &x]).unwrap()[0]["claim"].clone();
    assert_eq!(first["generation"].as_i64(), Some(1));
    let refused = b.run(["claim", &x, "--json"]);
    assert_eq!(code(&refused), "ALREADY_CLAIMED"); // b never synced by hand
    assert_eq!(b.show(&x)["claim"]["owner"], first["owner"]);
    // The same instant, from two machines, the way agents usually claim.
    let attempts = pool_map(2, &[&a, &b], |m| {
        m.run(["update", &y, "--status", "in_progress", "--json"])
    });
    assert_eq!(sorted_codes(&attempts), [0, 1]);
    let refused = attempts.iter().find(|p| p.returncode() != 0).unwrap();
    assert_eq!(code(refused), "ALREADY_CLAIMED");
    settle(&[&a, &b]);
    assert_eq!(a.show(&y)["claim"], b.show(&y)["claim"]);
    // the refused attempt left no trace in the journal
    assert!(a.show(&y).get("lost_claims").is_none());
}

/// A claim lasts until its owner releases the issue or closes it: nobody else may close or release
/// it, a release opens the issue again and the next claim is the next generation, and a close
/// ends it everywhere. An issue cannot be created already in progress.
#[test]
fn claim_lasts_until_release_close_or_takeover() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let shortcut = a.run(["create", "shortcut", "--status", "in_progress", "--json"]);
    assert_eq!(code(&shortcut), "VALIDATION");
    a.bead(["claim", &x]);
    assert_eq!(claim_rows(&b), []);
    b.bead(["sync"]);
    let confirmed = (
        Json::from(x.as_str()),
        Json::from("confirmed"),
        Json::from(1),
    );
    assert_eq!(claim_rows(&b), [confirmed]);
    assert_eq!(code(&b.run(["close", &x, "--json"])), "CLAIMED_BY_OTHER");
    assert_eq!(code(&b.run(["release", &x, "--json"])), "NOT_OWNER");
    a.bead(["release", &x, "--reason", "parking it"]);
    assert_eq!(a.show(&x)["status"].as_str(), Some("open"));
    let claimed = b.bead(["claim", &x]).unwrap();
    assert_eq!(claimed[0]["claim"]["generation"].as_i64(), Some(2));
    b.bead(["close", &x, "--reason", "done"]);
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        assert_eq!(machine.show(&x)["status"].as_str(), Some("closed"));
        let claims = machine.bead(["claims"]).unwrap();
        assert_eq!(claims["claims"], Json::Array(Vec::new()));
    }
}

/// A takeover needs a reason, and fences the previous owner: its closes, one written before the
/// takeover and published after it, one written after, are stale everywhere, and the issues stay
/// with the new owner, whose own close counts.
#[test]
fn takeover_fences_the_previous_owner() {
    let sandbox = crate::sandbox();
    let (a, b, x, y) = two_machines(&sandbox);
    a.bead(["claim", &x]);
    a.bead(["claim", &y]);
    let unexplained = b.run(["takeover", &x, "--json"]);
    assert_eq!(code(&unexplained), "VALIDATION"); // a reason is required
    // written before the takeover, not yet published
    a.bead(["close", &y, "--reason", "finished", "--offline"]);
    for key in [&x, &y] {
        let takeover = ["takeover", key, "--reason", "owner crashed; continuing"];
        let taken = b.cmd(takeover).thread("b-developer").json().unwrap();
        assert_eq!(taken[0]["claim"]["generation"].as_i64(), Some(2));
    }
    // The old owner comes back and acts on what it still believes.
    let closed = a.run(["close", &x, "--reason", "finished", "--json"]);
    assert_eq!(closed.returncode(), 0, "{}", closed.stderr);
    assert_eq!(code(&a.run(["release", &x, "--json"])), "NOT_OWNER");
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        for key in [&x, &y] {
            // neither close took effect, whichever was written first
            let issue = machine.show(key);
            assert_eq!(issue["status"].as_str(), Some("in_progress"));
            assert_eq!(issue["claim"]["generation"].as_i64(), Some(2));
            assert!(owner(&issue).contains("b-develo"), "{issue:?}");
            assert_eq!(column(&issue["stale"], "op"), [Json::from("close")]);
        }
    }
    let done = ["close", &x, "--reason", "done"];
    b.cmd(done).thread("b-developer").json();
    settle(&[&a, &b]);
    assert_eq!(a.show(&x)["status"].as_str(), Some("closed"));
}

/// Offline claims are provisional: two of them leave the issue in progress with no owner; a
/// claim made online gives it one; and a provisional claim never displaces a confirmed owner.
#[test]
fn offline_claims_are_provisional() {
    let sandbox = crate::sandbox();
    let (a, b, x, y) = two_machines(&sandbox);
    a.bead(["claim", &x, "--offline"]);
    b.bead(["claim", &x, "--offline"]);
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        let issue = machine.show(&x);
        assert!(issue.get("claim").is_none(), "{issue:?}"); // nobody owns it
        assert_eq!(length(&issue["provisional_claims"]), 2);
        assert_eq!(issue["status"].as_str(), Some("in_progress"));
    }
    a.bead(["claim", &x]); // reconnecting means claiming normally
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        let issue = machine.show(&x);
        assert!(owner(&issue).contains("a-thread"), "{issue:?}");
        assert_eq!(length(&issue["provisional_claims"]), 1);
    }
    a.bead(["claim", &y]);
    b.bead(["claim", &y, "--offline"]); // b cannot see a's claim yet
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        // a provisional claim never displaces a confirmed owner
        let issue = machine.show(&y);
        assert!(owner(&issue).contains("a-thread"), "{issue:?}");
        assert_eq!(length(&machine.show(&y)["provisional_claims"]), 1);
    }
}

/// Of two machines claiming one issue at once, one wins, and the refused attempt leaves no trace
/// anywhere: one claim in the log, the winner's, no lost claim and no integrity problem, on every
/// machine.
#[test]
fn a_refused_claim_leaves_no_trace_anywhere() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let attempts = pool_map(2, &[&a, &b], |m| m.run(["claim", &x, "--json"]));
    assert_eq!(sorted_codes(&attempts), [0, 1]);
    let winner = if attempts[0].returncode() != 0 {
        &b
    } else {
        &a
    };
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        let log = machine.bead(["log", &x]).unwrap();
        let claims: Vec<&Json> = log
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["op"].as_str() == Some("claim"))
            .collect();
        // the refused attempt was taken back, not recorded
        assert_eq!(claims.len(), 1, "{log:?}");
        let thread = claims[0]["thread"].as_str().unwrap();
        let wanted = format!("{}-thread", winner.name());
        assert!(thread.contains(&wanted), "{thread}");
        assert_eq!(machine.show(&x)["claim"], winner.show(&x)["claim"]);
        assert!(machine.show(&x).get("lost_claims").is_none());
        let info = machine.bead(["info"]).unwrap();
        assert_eq!(info["integrity"], Json::Array(Vec::new()));
    }
}

/// Closing an issue through `update --status closed` publishes the close and ends the claim, as
/// `close` does.
#[test]
fn closing_through_update_publishes_like_close() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    a.bead(["claim", &x]);
    a.bead([
        "update",
        &x,
        "--status",
        "closed",
        "--notes",
        "done via update",
    ]);
    b.bead(["sync"]);
    assert_eq!(b.show(&x)["status"].as_str(), Some("closed"));
    assert!(b.show(&x).get("claim").is_none());
}

/// A claim needs the remote: when it cannot be reached the claim fails and claims nothing, unless
/// `--offline` asks for a provisional claim.
#[test]
fn claim_needs_the_remote_unless_offline_is_asked_for() {
    let sandbox = crate::sandbox();
    let (a, _, x, _) = two_machines(&sandbox);
    let gone = sandbox.root().join("unreachable.git");
    let gone = gone.to_str().unwrap();
    let refused = a.run(["claim", &x, "--remote", gone, "--json"]);
    assert_eq!(code(&refused), "REMOTE_UNREACHABLE");
    assert!(a.show(&x).get("claim").is_none());
    let claimed = a.bead(["claim", &x, "--offline"]).unwrap();
    assert_eq!(length(&claimed[0]["provisional_claims"]), 1);
}
