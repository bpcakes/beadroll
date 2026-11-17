//! Several machines' ordinary edits, merged: what converges, what is flagged as a conflict, and
//! what settles it.

use std::collections::BTreeSet;

use beadroll_test_support::{Json, Machine, code, git, json};

use super::{column, id, pool_map, settle, two_machines};

/// The text of every value in `values`.
fn texts(values: &[Json]) -> BTreeSet<String> {
    let text = |value: &Json| {
        value
            .as_str()
            .unwrap_or_else(|| panic!("{value:?}"))
            .to_owned()
    };
    values.iter().map(text).collect()
}

/// Two machines that changed the same issues converge: a field only one of them changed takes its
/// value, labels and comments from both are kept, and a field both changed shows the newer value,
/// flagged with both, until a write that has seen both settles it. Checkouts stay clean, and the
/// journal is not a branch.
#[test]
fn two_machines_converge_and_flag_what_both_changed() {
    let sandbox = crate::sandbox();
    let (a, b, x, y) = two_machines(&sandbox);
    assert_eq!(b.show(&x)["description"].as_str(), Some("original"));
    // Both work offline on the same issues.
    a.bead([
        "update",
        &x,
        "--title",
        "title from a",
        "--priority",
        "0",
        "--add-label",
        "from-a",
    ]);
    a.bead(["comments", "add", &x, "note from a"]);
    b.bead([
        "update",
        &x,
        "--description",
        "body from b",
        "--priority",
        "3",
        "--add-label",
        "from-b",
    ]);
    b.bead(["comments", "add", &x, "note from b"]);
    b.bead(["close", &y, "--reason", "done on b"]);
    a.bead(["comments", "add", &y, "late note from a"]);
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        let issue = machine.show(&x);
        assert_eq!(issue["title"].as_str(), Some("title from a")); // only a touched it
        assert_eq!(issue["description"].as_str(), Some("body from b")); // only b touched it
        assert_eq!(issue["priority"].as_i64(), Some(3)); // shown: the newer of the two
        let flagged = issue["conflicts"].as_object().unwrap();
        let flagged: BTreeSet<&str> = flagged.keys().map(String::as_str).collect();
        assert_eq!(flagged, BTreeSet::from(["priority"])); // and flagged, with both values
        let values = column(&issue["conflicts"]["priority"], "value");
        assert_eq!(values, [0, 3].map(Json::from));
        assert_eq!(issue["labels"], json!(["from-a", "from-b"]));
        let comments = column(&issue["comments"], "text");
        assert_eq!(comments, ["note from a", "note from b"].map(Json::from));
        let closed = machine.show(&y);
        assert_eq!(closed["status"].as_str(), Some("closed"));
        let comments = column(&closed["comments"], "text");
        assert_eq!(comments, [Json::from("late note from a")]);
        let listed = machine.bead(["list"]).unwrap();
        assert_eq!(listed["issues"][0]["conflicted"], json!(["priority"]));
    }
    a.bead(["update", &x, "--priority", "1"]); // a write that has seen both settles it
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        assert_eq!(machine.show(&x)["priority"].as_i64(), Some(1));
        assert!(machine.show(&x).get("conflicts").is_none());
    }
    for cwd in [a.checkout(), b.checkout()] {
        assert_eq!(git(["status", "--porcelain"], Some(cwd)), "");
        let branches = git(["branch", "-a"], Some(cwd));
        // the journal is not a branch
        assert_eq!(branches.matches("beads").count(), 0, "{branches}");
    }
}

/// An edit made after syncing another machine's edit of the same field replaces it, with no
/// conflict.
#[test]
fn an_edit_made_after_seeing_another_is_not_a_conflict() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    a.bead(["update", &x, "--priority", "0"]);
    a.bead(["sync"]);
    b.bead(["sync"]);
    b.bead(["update", &x, "--priority", "3"]);
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        assert_eq!(machine.show(&x)["priority"].as_i64(), Some(3));
        assert!(machine.show(&x).get("conflicts").is_none());
    }
}

/// Removing a label removes only the additions the remover has seen: the label another machine
/// added again meanwhile stays, until a removal that has seen that addition too.
#[test]
fn removing_a_label_only_removes_what_the_remover_saw() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    a.bead(["update", &x, "--add-label", "urgent"]);
    settle(&[&a, &b]);
    b.bead(["update", &x, "--add-label", "urgent"]); // b re-asserts it
    a.bead(["update", &x, "--remove-label", "urgent"]); // a removes the one addition it has seen
    settle(&[&a, &b]);
    assert_eq!(a.show(&x)["labels"], json!(["urgent"]));
    a.bead(["update", &x, "--remove-label", "urgent"]); // now a has seen every addition
    settle(&[&a, &b]);
    assert!(b.show(&x).get("labels").is_none());
}

/// Machines that set an issue's status differently flag it with both values; until a write that
/// has seen both settles it, neither the issue nor what waits for it is ready, and it cannot be
/// claimed.
#[test]
fn status_disagreement_holds_dependents_until_settled() {
    let sandbox = crate::sandbox();
    let (a, b, x, y) = two_machines(&sandbox);
    a.bead(["dep", "add", &y, &x]);
    settle(&[&a, &b]);
    a.bead(["close", &x, "--reason", "done"]);
    b.bead(["update", &x, "--status", "deferred"]);
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        let values = column(&machine.show(&x)["conflicts"]["status"], "value");
        let expected = BTreeSet::from(["closed", "deferred"].map(String::from));
        assert_eq!(texts(&values), expected);
        assert_eq!(machine.ready(), BTreeSet::new()); // x is disputed, and y still waits for it
    }
    assert_eq!(code(&a.run(["claim", &x, "--json"])), "STATUS_CONFLICT");
    a.bead(["close", &x, "--reason", "confirmed done"]);
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        assert!(machine.show(&x).get("conflicts").is_none());
        assert_eq!(machine.ready(), BTreeSet::from([y.clone()]));
    }
}

/// Two machines that give their offline children of one parent the same number: the sync that
/// brings them together renumbers one, and every machine agrees which child is which.
#[test]
fn same_child_number_on_two_machines_is_renumbered_identically() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let (first, second) = (format!("{x}.1"), format!("{x}.2"));
    let on_a = ["create", "child made on a", "--parent", &x, "--offline"];
    assert_eq!(id(&a.bead(on_a).unwrap()), first);
    let on_b = ["create", "child made on b", "--parent", &x, "--offline"];
    assert_eq!(id(&b.bead(on_b).unwrap()), first);
    a.bead(["sync"]);
    let report = b.bead(["sync"]).unwrap();
    let renamed: Vec<(Json, Json)> = report["renamed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["from"].clone(), r["to"].clone()))
        .collect();
    let expected = (Json::from(first.as_str()), Json::from(second.as_str()));
    assert_eq!(renamed, [expected]);
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        let made_on_a = machine.show(&first);
        assert_eq!(made_on_a["title"].as_str(), Some("child made on a"));
        let made_on_b = machine.show(&second);
        assert_eq!(made_on_b["title"].as_str(), Some("child made on b"));
        assert_eq!(machine.show(&second)["parent"].as_str(), Some(x.as_str()));
    }
}

/// Three machines syncing at the same moment all succeed, and converge on everything they made.
#[test]
fn concurrent_sync_from_three_machines_all_succeed() {
    let sandbox = crate::sandbox();
    let (a, b, _, _) = two_machines(&sandbox);
    let c = Machine::new(&sandbox, "c");
    c.bead(["sync"]);
    for machine in [&a, &b, &c] {
        for n in 0..3 {
            let title = format!("{} item {n}", machine.name());
            machine.bead(["create", &title]);
        }
    }
    let codes = pool_map(3, &[&a, &b, &c], |m| m.run(["sync", "--json"]).returncode());
    assert_eq!(codes, [0, 0, 0]);
    settle(&[&a, &b, &c]);
    let listed = a.bead(["list", "--limit", "0"]).unwrap();
    assert_eq!(listed["total"].as_i64(), Some(11));
}
