//! A takeover against the owner's concurrent writes: it fences what the owner did offline.

use beadroll_test_support::{Machine, printed_id};

use crate::commands::fixtures::machine;

/// E03: a takeover must fence the prior owner's stale terminal transition.
#[test]
fn takeover_fences_an_owners_concurrent_offline_delete() {
    let sandbox = crate::sandbox();
    let owner = machine(&sandbox, "e03-owner");
    let key = printed_id(owner.bead(["create", "terminal race"]));
    owner.bead(["sync"]);
    let successor = Machine::new(&sandbox, "e03-successor");
    successor.bead(["sync"]);
    owner.bead(["claim", &key]);

    owner.bead(["delete", &key, "--offline"]);
    successor.bead(["takeover", &key, "--reason", "continue work"]);
    owner.bead(["sync"]);
    successor.bead(["sync"]);

    let shown = successor.show(&key);
    assert_eq!(shown["status"].as_str(), Some("in_progress"), "{shown:?}");
    // No conflicts at all, or none about the status.
    let conflicts = shown
        .get("conflicts")
        .map(|c| c.as_object().expect("conflicts by field"));
    assert!(
        !conflicts.is_some_and(|c| c.contains_key("status")),
        "{shown:?}"
    );
    let claimant = shown["claim"]["owner"].as_str().expect("a claim's owner");
    assert!(claimant.contains("e03-successor-thread"), "{claimant}");
}
