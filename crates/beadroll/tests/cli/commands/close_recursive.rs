//! `bead close --recursive ID...` closes each named issue and every issue beneath it through
//! parent-child edges in one command, children first. Issues beneath that are deleted, or already
//! closed with nothing to end, are left as they are; another owner's claim anywhere refuses the
//! whole command unless `--force`. Only `close` takes the flag.

use beadroll_test_support::{Json, Machine, Sandbox, code};

/// Runs `bead ARGS` on `machine`, which must succeed: its standard output.
fn ok(machine: &Machine, args: &[&str]) -> String {
    let run = machine.run(args);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    run.stdout
}

/// Creates an issue titled `title` on `machine`, beneath `parent` when given: its key.
fn create(machine: &Machine, title: &str, parent: Option<&str>) -> String {
    let mut args = vec!["create", title];
    args.extend(
        parent
            .map(|parent| ["--parent", parent])
            .into_iter()
            .flatten(),
    );
    let made = machine.bead(&args).expect("create prints the issue");
    made["id"].as_str().unwrap().to_owned()
}

/// Machine a with tracker `ex` and the tree epic E, its children A and B (in that order), and A's
/// child G: the keys E, A, B, G.
fn tree(sandbox: &Sandbox) -> (Machine, [String; 4]) {
    tree_on(Machine::new(sandbox, "a"))
}

/// `tree` on `a`.
fn tree_on(a: Machine) -> (Machine, [String; 4]) {
    a.bead(["init", "--prefix", "ex"]);
    let e = create(&a, "epic", None);
    ok(&a, &["update", &e, "--type", "epic"]);
    let child_a = create(&a, "child a", Some(&e));
    let child_b = create(&a, "child b", Some(&e));
    let g = create(&a, "grandchild", Some(&child_a));
    (a, [e, child_a, child_b, g])
}

/// The issue and operation of every `close` in `bead log`, in its order.
fn closes(machine: &Machine) -> Vec<String> {
    let log = machine.bead(["log"]).unwrap();
    let rows = log.as_array().unwrap();
    rows.iter()
        .filter(|row| row["op"].as_str() == Some("close"))
        .map(|row| row["issue"].as_str().unwrap().to_owned())
        .collect()
}

/// The status `machine` shows for `key`.
fn status(machine: &Machine, key: &str) -> String {
    machine.show(key)["status"].as_str().unwrap().to_owned()
}

/// The ids of a `--json` answer, in its order.
fn ids(answer: &Json) -> Vec<String> {
    let items = answer.as_array().unwrap();
    items
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn closes_the_subtree_children_first_in_one_command_and_publishes_it() {
    let sandbox = crate::sandbox();
    let (a, [e, child_a, child_b, g]) = tree(&sandbox);
    let said = ok(&a, &["close", "--recursive", &e, "-r", "all done"]);
    assert_eq!(
        said.lines().collect::<Vec<_>>(),
        [
            format!("✓ Closed {g}: grandchild"),
            format!("✓ Closed {child_a}: child a"),
            format!("✓ Closed {child_b}: child b"),
            format!("✓ Closed {e}: epic"),
        ]
    );
    assert_eq!(
        closes(&a),
        [g.as_str(), child_a.as_str(), child_b.as_str(), e.as_str()]
    );
    for key in [&e, &child_a, &child_b, &g] {
        assert_eq!(a.show(key)["close_reason"].as_str(), Some("all done"));
    }
    // published as one local write: another machine sees all four after one sync
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    for key in [&e, &child_a, &child_b, &g] {
        assert_eq!(status(&b, key), "closed", "{key}");
    }
}

#[test]
fn without_the_flag_close_closes_only_what_it_names() {
    let sandbox = crate::sandbox();
    let (a, [e, child_a, child_b, g]) = tree(&sandbox);
    ok(&a, &["close", &e]);
    assert_eq!(status(&a, &e), "closed");
    for key in [&child_a, &child_b, &g] {
        assert_eq!(status(&a, key), "open", "{key}");
    }
    // each id is still checked in turn: a claim refuses before a later id that names nothing
    ok(&a, &["claim", &child_b, "--actor", "other"]);
    let refused = a.run(["close", &child_a, &child_b, "ex-missing", "--json"]);
    assert_eq!(code(&refused), "CLAIMED_BY_OTHER", "{}", refused.stderr);
    let refused = a.run(["close", &child_a, "ex-missing", &child_b, "--json"]);
    assert_eq!(code(&refused), "ISSUE_NOT_FOUND", "{}", refused.stderr);
    assert_eq!(status(&a, &child_a), "open");
}

#[test]
fn issues_beneath_already_closed_or_deleted_are_left_as_they_are() {
    let sandbox = crate::sandbox();
    let (a, [e, child_a, child_b, g]) = tree(&sandbox);
    let gone = create(&a, "deleted beneath", Some(&child_b));
    ok(&a, &["delete", &gone]);
    ok(&a, &["close", &g, "-r", "earlier"]);
    let said = ok(&a, &["close", "--recursive", &e]);
    assert_eq!(
        said.lines().collect::<Vec<_>>(),
        [
            format!("✓ Closed {child_a}: child a"),
            format!("✓ Closed {child_b}: child b"),
            format!("✓ Closed {e}: epic"),
            "2 issues beneath were already closed or deleted: left as they are".to_owned(),
        ]
    );
    // closing a deleted issue by name brings it back; beneath, it stays deleted
    assert_eq!(status(&a, &gone), "tombstone");
    assert_eq!(a.show(&g)["close_reason"].as_str(), Some("earlier"));
    assert_eq!(
        closes(&a),
        [g.as_str(), child_a.as_str(), child_b.as_str(), e.as_str()]
    );

    // again: the named one answers as a plain close does, and nothing is written
    let again = a.bead(["close", "--recursive", &e]).unwrap();
    assert_eq!(ids(&again), [e.as_str()]);
    assert_eq!(again[0]["unchanged"].as_str(), Some("already closed"));
    let said = ok(&a, &["close", "--recursive", &e]);
    assert_eq!(
        said.lines().collect::<Vec<_>>(),
        [
            format!("✓ Already closed {e}: epic"),
            "4 issues beneath were already closed or deleted: left as they are".to_owned(),
        ]
    );
    assert_eq!(closes(&a).len(), 4);
}

#[test]
fn another_owners_claim_beneath_refuses_all_and_names_each_until_forced() {
    let sandbox = crate::sandbox();
    let (a, [e, child_a, child_b, g]) = tree(&sandbox);
    ok(&a, &["claim", &child_b, "--actor", "other"]);
    ok(&a, &["claim", &g, "--actor", "other"]);
    let events = a.bead(["log"]).unwrap().as_array().unwrap().len();
    let refused = a.run(["close", "--recursive", &e, "--json"]);
    assert_eq!(refused.returncode(), 1, "{}", refused.stderr);
    assert_eq!(code(&refused), "CLAIMED_BY_OTHER");
    for key in [&g, &child_b] {
        assert!(
            refused
                .stderr
                .contains(&format!("{key} is claimed by other")),
            "{}",
            refused.stderr
        );
    }
    // nothing written
    assert_eq!(a.bead(["log"]).unwrap().as_array().unwrap().len(), events);
    for key in [&e, &child_a] {
        assert_eq!(status(&a, key), "open", "{key}");
    }

    let forced = a.bead(["close", "--recursive", &e, "--force"]).unwrap();
    assert_eq!(
        ids(&forced),
        [g.as_str(), child_a.as_str(), child_b.as_str(), e.as_str()]
    );
    for key in [&child_b, &g] {
        assert_eq!(status(&a, key), "closed", "{key}");
        assert!(a.show(key)["claim"] == Json::Null, "{key}");
    }
}

#[test]
fn this_actors_claim_beneath_ends_with_the_close() {
    let sandbox = crate::sandbox();
    let (a, [e, child_a, _, _]) = tree(&sandbox);
    ok(&a, &["claim", &child_a]);
    let closed = a.bead(["close", "--recursive", &e]).unwrap();
    assert!(
        closed
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("unpublished").is_none()),
        "{closed:?}"
    );
    assert!(a.show(&child_a)["claim"] == Json::Null);
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    assert_eq!(status(&b, &child_a), "closed");
    assert!(b.show(&child_a)["claim"] == Json::Null);
}

#[test]
fn a_retry_of_an_unpublished_recursive_close_publishes_the_claim_it_ended() {
    let sandbox = crate::sandbox();
    let (a, [e, child_a, child_b, g]) = tree_on(crate::rust_only(&sandbox, "a"));
    ok(&a, &["claim", &child_a]);
    let b = Machine::new(&sandbox, "b");
    // written here, then stopped before its push
    let stopped = a
        .cmd(["close", "--recursive", &e])
        .env("BEAD_FAILPOINTS", "push.before=exit(98)")
        .run();
    assert_eq!(stopped.returncode(), 98, "{}", stopped.stderr);
    b.bead(["sync"]);
    assert_eq!(status(&b, &child_a), "in_progress");
    // everything is closed here now, so the retry writes nothing; it still publishes the claim
    // it ended
    let again = ok(&a, &["close", "--recursive", &e]);
    assert!(
        again.contains("3 issues beneath were already closed or deleted"),
        "{again}"
    );
    b.bead(["sync"]);
    for key in [&e, &child_a, &child_b, &g] {
        assert_eq!(status(&b, key), "closed", "{key}");
    }
    assert_eq!(b.show(&child_a)["claim"], Json::Null);
}

#[test]
fn overlapping_roots_close_each_issue_once_and_a_second_parent_outside_stays_open() {
    let sandbox = crate::sandbox();
    let (a, [e, child_a, child_b, g]) = tree(&sandbox);
    let outsider = create(&a, "outsider", None);
    let shared = create(&a, "shared", Some(&child_a));
    ok(
        &a,
        &["dep", "add", &shared, &outsider, "--type", "parent-child"],
    );
    let closed = a
        .bead(["close", "--recursive", &e, &child_a, &shared])
        .unwrap();
    assert_eq!(
        ids(&closed),
        [
            g.as_str(),
            shared.as_str(),
            child_a.as_str(),
            child_b.as_str(),
            e.as_str()
        ]
    );
    assert_eq!(
        closes(&a),
        [
            g.as_str(),
            shared.as_str(),
            child_a.as_str(),
            child_b.as_str(),
            e.as_str()
        ]
    );
    assert_eq!(status(&a, &outsider), "open");
}

#[test]
fn a_parent_loop_merged_from_two_machines_ends() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let k = create(&a, "k", None);
    let m = create(&a, "m", None);
    let z = create(&a, "beneath m", Some(&m));
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    // each makes one the other's parent, unheard: merged, the two are each other's child
    ok(
        &a,
        &["dep", "add", &m, &k, "--type", "parent-child", "--offline"],
    );
    ok(
        &b,
        &["dep", "add", &k, &m, "--type", "parent-child", "--offline"],
    );
    a.bead(["sync"]);
    b.bead(["sync"]);
    a.bead(["sync"]);
    let cycles = a.bead(["dep", "cycles"]).unwrap();
    assert_eq!(cycles["count"].as_i64(), Some(1), "{cycles:?}");
    let closed = a.bead(["close", "--recursive", &k]).unwrap();
    assert_eq!(ids(&closed), [z.as_str(), m.as_str(), k.as_str()]);
    assert_eq!(closes(&a), [z.as_str(), m.as_str(), k.as_str()]);
}

#[test]
fn offline_it_is_recorded_and_said_to_be_unpublished() {
    let sandbox = crate::sandbox();
    let (a, [e, child_a, child_b, g]) = tree(&sandbox);
    let said = ok(&a, &["close", "--recursive", &e, "--offline"]);
    assert!(
        said.lines()
            .any(|line| line == "4 events not yet published: bead sync"),
        "{said}"
    );
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    for key in [&e, &child_a, &child_b, &g] {
        assert_eq!(status(&b, key), "open", "{key}");
    }
}

#[test]
fn only_close_takes_the_flag() {
    let sandbox = crate::sandbox();
    let (a, [e, ..]) = tree(&sandbox);
    for op in ["reopen", "delete"] {
        let refused = a.run([op, "--recursive", &e, "--json"]);
        assert_eq!(refused.returncode(), 2, "{op}: {}", refused.stderr);
        assert_eq!(code(&refused), "USAGE", "{op}");
    }
    assert_eq!(status(&a, &e), "open");
}
