//! The dependency graph: what is ready, and the cycles bead refuses or, when a merge forms one,
//! reports.

use std::collections::BTreeSet;

use beadroll_test_support::{Json, Machine, code};

use super::{id, settle, two_machines};

/// Readiness follows the rule br uses: an epic waits for its open children and any other parent
/// does not; an issue held by its prerequisite holds everything beneath it until the prerequisite
/// closes; and a dependency the graph already has the other way round is refused as a cycle.
#[test]
fn readiness_follows_the_rule_br_uses() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let create = |args: &[&str]| id(&a.bead(args).unwrap());
    let gate = create(&["create", "prerequisite"]);
    let epic = create(&["create", "epic", "-t", "epic"]);
    let feature = create(&[
        "create",
        "feature with a subtask",
        "-t",
        "feature",
        "--parent",
        &epic,
        "--deps",
        &gate,
    ]);
    let subtask = create(&["create", "subtask", "--parent", &feature]);
    let plain = create(&["create", "plain parent", "-t", "feature"]);
    a.bead(["create", "its child", "--parent", &plain]);
    assert!(!a.ready().contains(&epic)); // an epic waits for its open children
    assert!(a.ready().contains(&plain)); // any other parent does not
    assert!(!a.ready().contains(&feature)); // held by its prerequisite
    assert!(!a.ready().contains(&subtask)); // and so is everything beneath it
    a.bead(["close", &gate, "--reason", "done"]);
    let ready = a.ready();
    assert!(
        BTreeSet::from([feature, subtask.clone()]).is_subset(&ready),
        "{ready:?}"
    );
    let cycle = a.run(["dep", "add", &subtask, &epic, "--json"]);
    assert_eq!(code(&cycle), "CYCLE"); // epic already waits for it
}

/// An edge refused locally because it would close a cycle can still be added on a machine that
/// has not seen the other edge; once the two merge, every machine reports the cycle, and nothing
/// is ready.
#[test]
fn cycle_formed_by_a_merge_is_reported_and_never_ready() {
    let sandbox = crate::sandbox();
    let (a, b, x, y) = two_machines(&sandbox);
    a.bead(["dep", "add", &x, &y]);
    assert_eq!(code(&a.run(["dep", "add", &y, &x, "--json"])), "CYCLE"); // refused locally
    b.bead(["dep", "add", &y, &x]); // valid on b, which has not seen a's edge
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        let cycles = machine.bead(["dep", "cycles"]).unwrap();
        assert_eq!(cycles["count"].as_i64(), Some(1));
        assert_eq!(machine.bead(["ready"]), Some(Json::Array(Vec::new())));
    }
}

/// A parent change that would make an issue its own parent, or the parent of its parent, is
/// refused as a cycle, and leaves no cycle behind.
#[test]
fn a_parent_change_cannot_form_a_loop() {
    let sandbox = crate::sandbox();
    let (a, _, x, y) = two_machines(&sandbox);
    let own = a.run(["update", &x, "--parent", &x, "--json"]);
    assert_eq!(code(&own), "CYCLE");
    a.bead(["update", &y, "--parent", &x]);
    let around = a.run(["update", &x, "--parent", &y, "--json"]);
    assert_eq!(code(&around), "CYCLE");
    let cycles = a.bead(["dep", "cycles"]).unwrap();
    assert_eq!(cycles["count"].as_i64(), Some(0));
}
