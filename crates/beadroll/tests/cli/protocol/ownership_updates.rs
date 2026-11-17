//! An `update` that ends or takes a claim behaves as the ownership command does: a forced close
//! through `update` publishes as `close` does (R12), and a claiming update that fails writes, claims
//! and publishes nothing, whichever of its targets it fails on (I06).

use beadroll_test_support::{Json, Machine, code};

use crate::commands::fixtures::published_issue;

/// `bead claims`' list of claims on `machine`.
fn claims(machine: &Machine) -> Json {
    let listed = machine.bead(["claims"]).expect("the claims");
    listed["claims"].clone()
}

/// R12: a machine that does not hold the claim force-closes the issue through
/// `update --status closed --force`: the claim ends, and the close is published at once, as
/// `close --force` publishes it, so the owner sees the issue closed after a sync.
#[test]
fn forced_close_through_update_publishes_like_forced_close() {
    let sandbox = crate::sandbox();
    let (a, key) = published_issue(&sandbox);
    a.bead(["claim", &key]);
    let b = Machine::new(&sandbox, "root2-b");
    b.bead(["sync"]);
    let close = ["update", &key, "--status", "closed", "--force"];
    let updated = b.bead(close).expect("the updated issue");
    let result = &updated[0];
    assert_eq!(result["status"].as_str(), Some("closed"));
    assert!(result.get("claim").is_none(), "{result:?}");
    a.bead(["sync"]);
    assert_eq!(
        a.show(&key)["status"].as_str(),
        Some("closed"),
        "forced update close was never published"
    );
}

/// I06: a claiming update of an issue and its child that would make the child the parent of both
/// fails with `CYCLE` before anything is written: the export is unchanged, and nothing is claimed,
/// here or on a machine that syncs afterwards.
#[test]
fn claim_update_cycle_validation_is_atomic_for_all_targets() {
    let sandbox = crate::sandbox();
    let (a, key) = published_issue(&sandbox);
    let created = a.bead(["create", "child", "--parent", &key]);
    let child = created.expect("the child")["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let before = a.export();
    let proc = a.run([
        "update",
        &key,
        &child,
        "--claim",
        "--parent",
        &child,
        "--title",
        "must not persist",
        "--json",
    ]);
    assert_eq!(proc.returncode(), 1, "{}", proc.stderr);
    assert_eq!(code(&proc), "CYCLE");
    assert_eq!(a.export(), before);
    assert_eq!(claims(&a), Json::Array(Vec::new()));
    let c = Machine::new(&sandbox, "root2-c");
    c.bead(["sync"]);
    assert_eq!(claims(&c), Json::Array(Vec::new()));
}

/// A claiming update refused because another machine holds the claim (`ALREADY_CLAIMED`) publishes
/// none of the field edits bundled with it: once both machines have synced, both show the original
/// title.
#[test]
fn failed_claim_does_not_publish_bundled_field_edit() {
    let sandbox = crate::sandbox();
    let (a, key) = published_issue(&sandbox);
    a.bead(["claim", &key]);
    let b = Machine::new(&sandbox, "root2-b");
    b.bead(["sync"]);
    let proc = b.run([
        "update",
        &key,
        "--claim",
        "--title",
        "must not persist",
        "--json",
    ]);
    assert_eq!(code(&proc), "ALREADY_CLAIMED");
    b.bead(["sync"]);
    a.bead(["sync"]);
    assert_eq!(a.show(&key)["title"].as_str(), Some("root re-audit"));
    assert_eq!(b.show(&key)["title"].as_str(), Some("root re-audit"));
}
