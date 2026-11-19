//! The dependency graph: the edges `dep` adds and takes back, what is ready, and the cycles bead
//! refuses or, when a merge forms one, reports.

use std::collections::BTreeSet;

use beadroll_test_support::{Json, Machine, code, json_id};

use super::{settle, two_machines};

/// Readiness follows the rule br uses: an epic waits for its open children and any other parent
/// does not; an issue held by its prerequisite holds everything beneath it until the prerequisite
/// closes; and a dependency the graph already has the other way round is refused as a cycle.
#[test]
fn readiness_follows_the_rule_br_uses() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let create = |args: &[&str]| json_id(&a.bead(args).unwrap());
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

/// A new issue whose parent, named by `--parent` or as a `parent-child` dependency, already waits on
/// one of its blocking dependencies would close a loop through it: refused as a cycle, as `dep add`
/// refuses the same edge, and no loop left behind (AT03).
#[test]
fn a_new_issue_cannot_close_a_loop_through_its_parent() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let x = json_id(&a.bead(["create", "x"]).expect("x"));
    let y = json_id(&a.bead(["create", "y"]).expect("y"));
    a.bead(["dep", "add", &y, &x]); // y waits for x
    // x as the new issue's parent waits for it, and it waits for y
    let as_dependency = format!("parent-child:{x},blocks:{y}");
    let blocking = format!("blocks:{y}");
    let ways: [&[&str]; 2] = [
        &["create", "new", "--deps", &as_dependency, "--json"],
        &[
            "create", "new", "--parent", &x, "--deps", &blocking, "--json",
        ],
    ];
    for args in ways {
        let looped = a.run(args);
        assert_eq!(code(&looped), "CYCLE", "{args:?}: {}", looped.stdout);
        let cycles = a.bead(["dep", "cycles"]).expect("the cycles");
        assert_eq!(cycles["count"].as_i64(), Some(0), "{args:?}: {cycles:?}");
    }
}

/// `dep add`, `dep remove` and `dep rm` each answer with the action by the word that asked for it,
/// and `rm` takes an edge back as `remove` does.
#[test]
fn dep_answers_with_the_action_word_it_was_given() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let x = json_id(&a.bead(["create", "x"]).unwrap());
    let y = json_id(&a.bead(["create", "y"]).unwrap());
    for (word, removing) in [
        ("add", false),
        ("remove", true),
        ("add", false),
        ("rm", true),
    ] {
        let answer = a.bead(["dep", word, &x, &y]).expect("an answer");
        assert_eq!(answer["action"], Json::from(word), "{answer:?}");
        assert_eq!(
            answer["depends_on_id"],
            Json::from(y.as_str()),
            "{answer:?}"
        );
        let listed = a.bead(["dep", "list", &x]).expect("a list");
        let depends = listed["dependencies"]
            .as_array()
            .map_or(0, |rows| rows.len());
        assert_eq!(depends, usize::from(!removing), "{word}: {listed:?}");
    }
}

/// An empty `--type` is no type given: `dep add` adds `blocks`, and `dep remove` takes back the
/// edge whatever its type, rather than matching an empty type no edge has and saying it removed it.
#[test]
fn an_empty_dependency_type_is_no_type_given() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let x = json_id(&a.bead(["create", "x"]).unwrap());
    let y = json_id(&a.bead(["create", "y"]).unwrap());
    let depends = |a: &Machine| -> Vec<String> {
        let listed = a.bead(["dep", "list", &x]).expect("a list");
        let rows = listed["dependencies"].as_array().unwrap_or_default();
        (rows.iter())
            .map(|row| {
                row["dependency_type"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    };
    a.bead(["dep", "add", &x, &y, "--type", ""]);
    assert_eq!(depends(&a), ["blocks"]);
    a.bead(["dep", "remove", &x, &y, "--type", ""]);
    assert_eq!(
        depends(&a),
        Vec::<String>::new(),
        "the edge was not taken back"
    );
}

/// `dep remove` of a dependency the issue does not have, of any type or of the one named, is
/// refused as `VALIDATION` and writes nothing, rather than saying it took back what was never there.
#[test]
fn removing_a_dependency_that_is_not_there_is_refused() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let x = json_id(&a.bead(["create", "x"]).unwrap());
    let y = json_id(&a.bead(["create", "y"]).unwrap());
    let events = |a: &Machine| a.bead(["info"]).unwrap()["events"].clone();
    let before = events(&a);
    let none = a.run(["dep", "remove", &x, &y, "--json"]);
    assert_eq!(code(&none), "VALIDATION", "{}", none.stderr);
    assert!(
        none.stderr.contains("does not depend on"),
        "{}",
        none.stderr
    );
    a.bead(["dep", "add", &x, &y]);
    let wrong = a.run(["dep", "remove", &x, &y, "--type", "related", "--json"]);
    assert_eq!(code(&wrong), "VALIDATION", "{}", wrong.stderr);
    let listed = a.bead(["dep", "list", &x]).expect("a list");
    assert_eq!(
        listed["dependencies"].as_array().map(<[_]>::len),
        Some(1),
        "the edge was lost"
    );
    a.bead(["dep", "remove", &x, &y]);
    let removed = a.run(["dep", "remove", &x, &y, "--json"]);
    assert_eq!(
        code(&removed),
        "VALIDATION",
        "a second removal: {}",
        removed.stderr
    );
    // the refusals wrote nothing: an add and one removal since
    assert_eq!(events(&a).as_i64(), before.as_i64().map(|n| n + 2));
}

/// An issue two machines end apart (closed on one, deleted on the other) has a disputed status,
/// which is not done until a write settles it, for `blocked` as for `show` and readiness: an open
/// blocker still holds it, and `blocked` lists it, though either status it shows is a done one.
#[test]
fn blocked_lists_an_issue_whose_ending_is_disputed() {
    let sandbox = crate::sandbox();
    let (a, b, x, gate) = super::two_machines(&sandbox);
    a.bead(["dep", "add", &x, &gate]);
    super::settle(&[&a, &b]);
    a.bead(["close", &x, "--reason", "done here", "--offline"]);
    b.bead(["delete", &x, "--reason", "not needed", "--offline"]);
    super::settle(&[&a, &b]);
    for machine in [&a, &b] {
        let shown = machine.show(&x);
        assert!(shown.get("conflicts").is_some(), "not disputed: {shown:?}");
        let status = shown["status"].as_str().unwrap_or_default();
        assert!(["closed", "tombstone"].contains(&status), "{shown:?}");
        assert_eq!(shown["blocked"], Json::Bool(true), "{shown:?}");
        let blocked = machine
            .bead(["blocked"])
            .expect("bead blocked prints a list");
        let listed: Vec<&str> = (blocked.as_array().expect("a list").iter())
            .filter_map(|row| row["id"].as_str())
            .collect();
        assert_eq!(listed, [x.as_str()], "{shown:?} {blocked:?}");
    }
}

/// A child inherits its ancestors' prerequisites, so a loop through them makes an issue wait for
/// itself though `dep cycles` finds no cycle: `update --parent`, `dep add --type parent-child` and a
/// `blocks` dependency on a descendant are refused as `CYCLE`, as `create` refused the same loop.
/// Parenting that closes no loop still goes through.
#[test]
fn a_loop_through_an_inherited_prerequisite_is_refused() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let made = |title: &str| json_id(&a.bead(["create", title]).unwrap());
    let (x, y) = (made("x"), made("y"));
    a.bead(["dep", "add", &x, &y]);
    let refused = [
        a.run(["update", &y, "--parent", &x, "--json"]),
        a.run(["dep", "add", &y, &x, "--type", "parent-child", "--json"]),
    ];
    for run in &refused {
        assert_eq!(code(run), "CYCLE", "{}", run.stderr);
    }
    let parent = made("parent");
    let child = json_id(&a.bead(["create", "child", "--parent", &parent]).unwrap());
    let grandchild = json_id(
        &a.bead(["create", "grandchild", "--parent", &child])
            .unwrap(),
    );
    for target in [&child, &grandchild] {
        let run = a.run(["dep", "add", &parent, target, "--json"]);
        assert_eq!(code(&run), "CYCLE", "{target}: {}", run.stderr);
    }
    let other = made("other");
    let moved = a.run(["update", &other, "--parent", &parent, "--json"]);
    assert_eq!(moved.returncode(), 0, "{}", moved.stderr);
    let blocked = a.bead(["blocked"]).unwrap();
    for row in blocked.as_array().unwrap() {
        let id = json_id(row);
        assert!(
            !row["blocked_by"]
                .as_array()
                .unwrap()
                .contains(&Json::from(id.as_str())),
            "{id} waits for itself"
        );
    }
}
