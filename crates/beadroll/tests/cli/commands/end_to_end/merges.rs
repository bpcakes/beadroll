//! Several machines' ordinary edits, merged: what converges, what is flagged as a conflict, and
//! what settles it.

use std::collections::BTreeSet;

use beadroll_test_support::{Json, Machine, code, git, json, json_id, pool_map};

use super::{column, settle, two_machines};

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
    assert_eq!(json_id(&a.bead(on_a).unwrap()), first);
    let on_b = ["create", "child made on b", "--parent", &x, "--offline"];
    assert_eq!(json_id(&b.bead(on_b).unwrap()), first);
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

/// An online child's create answers the key it has once its own publish has merged: another
/// machine's child of that number published first, the create's key is the next one, never the
/// one that now names the other machine's child, in text, `--json` and `--silent` alike.
#[test]
fn an_online_child_answers_the_key_its_publish_settled() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let first = format!("{x}.1");
    assert_eq!(
        json_id(&a.bead(["create", "on a", "--parent", &x]).unwrap()),
        first
    );
    let made = b.bead(["create", "on b", "--parent", &x]).unwrap();
    let key = json_id(&made);
    assert_ne!(
        key, first,
        "b's create answered the key of a's child: {made:?}"
    );
    assert_eq!(b.show(&key)["title"].as_str(), Some("on b"));
    let silent = b.run(["create", "also on b", "--parent", &x, "--silent"]);
    assert_eq!(
        b.show(silent.stdout.trim())["title"].as_str(),
        Some("also on b")
    );
}

/// Two machines that close one issue with different reasons, neither seeing the other's, dispute
/// its status as two different statuses do: `show` lists each close with its reason, what depends
/// on it is not ready, and a close made after seeing both settles it on every machine.
#[test]
fn closes_with_different_reasons_are_a_conflict_a_new_close_settles() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let after = json_id(&a.bead(["create", "after x"]).unwrap());
    a.bead(["dep", "add", &after, &x]);
    b.bead(["sync"]);
    a.bead(["close", &x, "-r", "fixed in abc", "--offline"]);
    b.bead(["close", &x, "-r", "duplicate of another"]);
    settle(&[&a, &b]);
    let shown = a.show(&x);
    let reasons: Vec<&str> = (shown["conflicts"]["status"].as_array())
        .unwrap_or_else(|| panic!("no status conflict: {shown:?}"))
        .iter()
        .map(|alternative| alternative["close_reason"].as_str().unwrap())
        .collect();
    assert_eq!(reasons.len(), 2, "{shown:?}");
    assert!(reasons.contains(&"fixed in abc") && reasons.contains(&"duplicate of another"));
    let ready = a.bead(["ready"]).unwrap();
    let ready: Vec<String> = ready.as_array().unwrap().iter().map(json_id).collect();
    assert!(!ready.contains(&after), "{after} is ready: {ready:?}");
    a.bead(["close", &x, "-r", "fixed in abc; the other is a duplicate"]);
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        let shown = machine.show(&x);
        assert!(shown.get("conflicts").is_none(), "{shown:?}");
        assert_eq!(
            shown["close_reason"],
            json!("fixed in abc; the other is a duplicate")
        );
    }
}

/// Two deletes with different reasons are one tombstone, each reason kept as the delete's own: no
/// dispute, and what depends on the issue is ready. Comparing a done status by its reason, as two
/// closes are, held a dependent up behind a deleted issue.
#[test]
fn deletes_with_different_reasons_are_one_tombstone() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let after = json_id(&a.bead(["create", "after x"]).unwrap());
    a.bead(["dep", "add", &after, &x]);
    b.bead(["sync"]);
    a.bead(["delete", &x, "-r", "a duplicate", "--offline"]);
    b.bead(["delete", &x, "-r", "obsolete"]);
    settle(&[&a, &b]);
    assert!(a.show(&x).get("conflicts").is_none(), "{:?}", a.show(&x));
    assert!(
        a.ready().contains(&after),
        "{after} is held behind a deleted issue"
    );
}

/// Two closes with the same reason agree: nothing is disputed.
#[test]
fn closes_with_the_same_reason_agree() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    a.bead(["close", &x, "-r", "done", "--offline"]);
    b.bead(["close", &x, "-r", "done"]);
    settle(&[&a, &b]);
    let shown = a.show(&x);
    assert!(shown.get("conflicts").is_none(), "{shown:?}");
}

/// A child made under a parent that a merge then renames follows it: `x.1.1`, made on b under b's
/// own `x.1`, which a's child of that number came first and made `x.2`, is `x.2.1`, and sync
/// reports both renames. It kept `x.1.1`, which read as a child of a's `x.1`, whose next child then
/// skipped a number.
#[test]
fn a_child_follows_its_parents_rename() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let first = format!("{x}.1");
    assert_eq!(
        json_id(&a.bead(["create", "on a", "--parent", &x]).unwrap()),
        first
    );
    let made = json_id(
        &b.bead(["create", "on b", "--parent", &x, "--offline"])
            .unwrap(),
    );
    let grand = json_id(
        &b.bead(["create", "under b's", "--parent", &made, "--offline"])
            .unwrap(),
    );
    assert_eq!(grand, format!("{first}.1"));
    let report = b.bead(["sync"]).unwrap();
    let renamed: Vec<(String, String)> = (report["renamed"].as_array().unwrap().iter())
        .map(|r| {
            (
                r["from"].as_str().unwrap().to_owned(),
                r["to"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let second = format!("{x}.2");
    assert_eq!(
        renamed,
        [
            (first.clone(), second.clone()),
            (format!("{first}.1"), format!("{second}.1"))
        ]
    );
    settle(&[&a, &b]);
    for machine in [&a, &b] {
        assert_eq!(
            machine.show(&format!("{second}.1"))["title"].as_str(),
            Some("under b's")
        );
        assert_eq!(
            machine.show(&format!("{second}.1"))["parent"].as_str(),
            Some(second.as_str())
        );
    }
    let next = json_id(&a.bead(["create", "next on a", "--parent", &first]).unwrap());
    assert_eq!(next, format!("{first}.1"), "a's child skipped a number");
}

/// A child follows the parent its writer meant when that writer saw two renames of one key: c saw
/// b's child as `x.1` and its own as `x.2`, made a child of `x.1`, and a's `x.1` then came first,
/// so b's became `x.2` and c's `x.3`. The child follows b's, its parent, to `x.2.1`, never c's.
#[test]
fn a_child_follows_the_parent_its_writer_meant_among_renames() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    let c = Machine::new(&sandbox, "c");
    c.bead(["sync"]);
    a.bead(["create", "on a", "--parent", &x, "--offline"]);
    b.bead(["create", "on b", "--parent", &x, "--offline"]);
    b.bead(["sync"]);
    c.bead(["create", "on c", "--parent", &x, "--offline"]);
    c.bead(["sync"]);
    assert_eq!(c.show(&format!("{x}.1"))["title"].as_str(), Some("on b"));
    let under = ["create", "under b's", "--parent", &format!("{x}.1")];
    assert_eq!(json_id(&c.bead(under).unwrap()), format!("{x}.1.1"));
    settle(&[&a, &b, &c]);
    // b's child, by its title: its key is the one the merge gave it
    let listed = a.bead(["list", "--all", "--limit", "0"]).unwrap();
    let key_of = |title: &str| {
        (listed["issues"].as_array().unwrap().iter())
            .find(|issue| issue["title"] == Json::from(title))
            .map(json_id)
            .unwrap_or_else(|| panic!("no {title} in {listed:?}"))
    };
    let (parent, child) = (key_of("on b"), key_of("under b's"));
    assert_eq!(
        child,
        format!("{parent}.1"),
        "the child follows another sibling"
    );
    for machine in [&a, &b, &c] {
        let shown = machine.show(&child);
        assert_eq!(shown["parent"].as_str(), Some(parent.as_str()), "{shown:?}");
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
