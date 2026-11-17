//! When an issue ends, so do its claims: no claim outlives a tombstone (Q05), and an offline
//! close ends the provisional claims it saw (Q11).

use std::path::PathBuf;

use beadroll_test_support::{Machine, code, json, printed_id};

use super::fixtures::glob;

/// Q05: a terminal tombstone transition must not survive with a live claim.
#[test]
fn q05_tombstone_update_cannot_leave_a_releasable_claim() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q05-tombstone");
    a.bead(["init", "--prefix", "ex"]);
    let key = printed_id(a.bead(["create", "delete me"]));
    a.bead(["sync"]);
    let result = a.run(["update", &key, "--claim", "--status", "tombstone", "--json"]);
    // this spelling is refused in favour of `delete`, and nothing of the claim is left anywhere
    assert_eq!(result.returncode(), 1, "{}", result.stdout);
    assert_eq!(code(&result), "VALIDATION");
    let shown = a.show(&key);
    assert_eq!(shown["status"], json!("open"));
    assert!(shown.get("claim").is_none(), "{shown:?}");
    assert_eq!(
        glob(&a.store().join("pending"), ".json"),
        Vec::<PathBuf>::new()
    );
    // events/*/claims/*.json
    let claims: Vec<PathBuf> = glob(&a.store().join("events"), "")
        .iter()
        .flat_map(|replica| glob(&replica.join("claims"), ".json"))
        .collect();
    assert_eq!(claims, Vec::<PathBuf>::new());
    let peer = Machine::new(&sandbox, "q05-rejected-peer");
    peer.bead(["sync"]);
    assert_eq!(peer.show(&key)["status"], json!("open"));
    assert!(peer.show(&key).get("claim").is_none());
}

/// Q11: close clears seen provisional claims but preserves concurrent ones.
#[test]
fn q11_offline_close_ends_a_provisional_claim() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q11-provisional");
    a.bead(["init", "--prefix", "ex"]);
    let key = printed_id(a.bead(["create", "offline work"]));
    a.bead(["sync"]);
    let concurrent = Machine::new(&sandbox, "q11-concurrent");
    concurrent.bead(["sync"]);

    // This claim has an earlier id but was made from a stale frontier, so A's later close did not
    // see it and must not erase it.
    concurrent.bead(["claim", &key, "--offline"]);
    a.bead(["claim", &key, "--offline"]);
    a.bead(["close", &key, "--offline"]);
    a.bead(["sync"]);
    concurrent.bead(["sync"]);
    a.bead(["sync"]);
    let shown = a.show(&key);
    assert_eq!(shown["status"], json!("closed"));
    let provisional = shown
        .get("provisional_claims")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let provisional = provisional.as_array().unwrap();
    assert_eq!(provisional.len(), 1, "{provisional:?}");
    let owner = provisional[0]["owner"].as_str().unwrap();
    assert!(owner.contains("q11-concurrent-thread"), "{owner}");
    let claims = a.bead(["claims"]).unwrap()["claims"].clone();
    let claims = claims.as_array().unwrap();
    assert_eq!(claims.len(), 1, "{claims:?}");
    let owner = claims[0]["owner"].as_str().unwrap();
    assert!(owner.contains("q11-concurrent-thread"), "{owner}");
}
