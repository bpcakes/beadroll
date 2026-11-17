//! `list`'s filters: an issue matches `--parent` through any of its parents.

use beadroll_test_support::{json_ids, printed_id};

use super::fixtures::machine;

/// E11: --parent must match any parent represented in the issue's dependency set.
#[test]
fn parent_filter_matches_every_direct_parent() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e11-parents");
    let first = printed_id(machine.bead(["create", "parent one"]));
    let second = printed_id(machine.bead(["create", "parent two"]));
    let child = printed_id(machine.bead(["create", "child"]));
    machine.bead(["dep", "add", &child, &first, "--type", "parent-child"]);
    machine.bead(["dep", "add", &child, &second, "--type", "parent-child"]);
    for parent in [&first, &second] {
        let result = machine
            .bead(["list", "--all", "--parent", parent, "--limit", "0"])
            .unwrap();
        let ids = json_ids(&result["issues"]);
        assert!(ids.contains(&child), "parent {parent}: {ids:?}");
    }
}
