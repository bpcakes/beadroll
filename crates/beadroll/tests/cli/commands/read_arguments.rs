//! AS18: a missing `add` must not silently turn supplied text or a dependency into a read.

use beadroll_test_support::{Machine, code};

/// Both human and JSON callers get an error, no answer, and the syntax they need.
fn rejected(machine: &Machine, args: &[&str], syntax: &str) {
    let human = machine.run(args);
    assert_eq!(human.returncode(), 1, "{args:?}: {human:?}");
    assert!(human.stdout.is_empty(), "{args:?}: {human:?}");
    assert!(human.stderr.contains(syntax), "{args:?}: {human:?}");

    let json = machine.run(args.iter().copied().chain(["--json"]));
    assert_eq!(json.returncode(), 1, "{args:?}: {json:?}");
    assert!(json.stdout.is_empty(), "{args:?}: {json:?}");
    assert_eq!(code(&json), "VALIDATION", "{args:?}");
    assert!(json.stderr.contains(syntax), "{args:?}: {json:?}");
}

#[test]
fn as18_comments_require_add_for_text_and_writing_options() {
    let sandbox = crate::sandbox();
    let machine = crate::rust_only(&sandbox, "comments");
    machine.bead(["init", "--prefix", "ex"]);
    let issue = machine.bead(["create", "Notes"]).unwrap();
    let key = issue["id"].as_str().unwrap();
    let before = machine.bead(["log", "--limit", "0"]).unwrap();

    for form in [vec!["comments", key], vec!["comments", "list", key]] {
        for extra in [
            vec!["a note"],
            vec!["several", "words"],
            vec![""],
            vec!["-m", "a note"],
            vec!["--message", ""],
            vec!["-f", "missing-comment-file"],
            vec!["--file", "-"],
            vec!["--author", "someone"],
        ] {
            let args: Vec<_> = form.iter().copied().chain(extra).collect();
            rejected(&machine, &args, "bead comments add ISSUE TEXT");
        }
    }
    for args in [
        vec!["comments"],
        vec!["comments", "list"],
        vec!["comments", "add"],
    ] {
        rejected(&machine, &args, "bead comments add ISSUE TEXT");
    }
    assert_eq!(machine.bead(["log", "--limit", "0"]).unwrap(), before);
    assert!(
        machine
            .bead(["comments", key])
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn as18_dependency_reads_reject_surplus_issues_and_writing_options() {
    let sandbox = crate::sandbox();
    let machine = crate::rust_only(&sandbox, "dependencies");
    machine.bead(["init", "--prefix", "ex"]);
    let a = machine.bead(["create", "A"]).unwrap();
    let b = machine.bead(["create", "B"]).unwrap();
    let a = a["id"].as_str().unwrap();
    let b = b["id"].as_str().unwrap();
    let before = machine.bead(["log", "--limit", "0"]).unwrap();

    for form in [vec!["dep", a], vec!["dep", "list", a]] {
        for extra in [vec![b], vec![""], vec!["--type", "blocks"]] {
            let args: Vec<_> = form.iter().copied().chain(extra).collect();
            rejected(&machine, &args, "bead dep add ISSUE DEPENDS_ON");
        }
    }
    rejected(&machine, &["dep", "tree", a, b], "bead dep tree ISSUE");
    rejected(&machine, &["dep", "cycles", a], "bead dep cycles");
    for form in [vec!["dep", "tree", a], vec!["dep", "cycles"]] {
        let args: Vec<_> = form.into_iter().chain(["--type", "blocks"]).collect();
        rejected(&machine, &args, "bead dep add ISSUE DEPENDS_ON");
    }
    assert_eq!(machine.bead(["log", "--limit", "0"]).unwrap(), before);
    assert!(
        machine.bead(["dep", a]).unwrap()["dependencies"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn comment_reads_and_explicit_add_keep_their_meaning() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "comments");
    machine.bead(["init", "--prefix", "ex"]);
    let issue = machine.bead(["create", "Notes"]).unwrap();
    let key = issue["id"].as_str().unwrap();
    for args in [vec!["comments", key], vec!["comments", "list", key]] {
        let run = machine.run(args);
        assert_eq!(run.returncode(), 0, "{run:?}");
        assert_eq!(run.stdout.trim(), "No comments");
    }
    machine.bead(["comments", "add", key, "a", "note"]);
    machine.bead([
        "comments",
        "add",
        key,
        "-m",
        "another note",
        "--author",
        "writer",
    ]);
    for args in [vec!["comments", key], vec!["comments", "list", key]] {
        let comments = machine.bead(args).unwrap();
        let comments = comments.as_array().unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0]["text"].as_str(), Some("a note"));
        assert_eq!(comments[1]["text"].as_str(), Some("another note"));
        assert_eq!(comments[1]["author"].as_str(), Some("writer"));
    }
}

#[test]
fn dependency_reads_and_explicit_changes_keep_their_meaning() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "dependencies");
    machine.bead(["init", "--prefix", "ex"]);
    let a = machine.bead(["create", "A"]).unwrap();
    let b = machine.bead(["create", "B"]).unwrap();
    let a = a["id"].as_str().unwrap();
    let b = b["id"].as_str().unwrap();
    machine.bead(["dep", "add", a, b, "--type", "blocks"]);
    for args in [vec!["dep", a], vec!["dep", "list", a]] {
        let listed = machine.bead(args).unwrap();
        let deps = listed["dependencies"].as_array().unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0]["id"].as_str(), Some(b));
        assert_eq!(deps[0]["dependency_type"].as_str(), Some("blocks"));
    }
    let tree = machine.bead(["dep", "tree", a]).unwrap();
    let tree = tree["tree"].as_array().unwrap();
    assert_eq!(tree.len(), 2);
    assert!(tree[0].as_str().unwrap().contains(a));
    assert!(tree[1].as_str().unwrap().contains(b));
    assert_eq!(
        machine.bead(["dep", "cycles"]).unwrap()["count"].as_i64(),
        Some(0)
    );
    machine.bead(["dep", "remove", a, b, "--type", "blocks"]);
    assert!(
        machine.bead(["dep", a]).unwrap()["dependencies"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
