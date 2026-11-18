//! AS18: a missing `add` must not silently turn supplied text or a dependency into a read.

use beadroll_test_support::{Machine, code, json};

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

/// AS18: without `add`, `comments ISSUE TEXT` and `dep A B` silently discarded input and exited 0;
/// explicit read forms and writing options on reads had the same gap.
#[test]
fn comments_require_add_for_text_and_writing_options() {
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

/// AS18: without `add`, `comments ISSUE TEXT` and `dep A B` silently discarded input and exited 0;
/// explicit read forms and writing options on reads had the same gap.
#[test]
fn dependency_reads_reject_surplus_issues_and_writing_options() {
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

/// A comment's text is given one way: words with `-m`, or `-m` with `-f`, is `USAGE` and writes
/// nothing, rather than keeping one and dropping the other without a word.
#[test]
fn a_comment_given_two_ways_is_refused() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "a");
    machine.bead(["init", "--prefix", "ex"]);
    let key = machine.bead(["create", "x"]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let file = sandbox.root().join("comment.txt");
    std::fs::write(&file, "from the file").unwrap();
    let file = file.to_str().unwrap();
    for args in [
        &[
            "comments",
            "add",
            key.as_str(),
            "first",
            "words",
            "-m",
            "second",
        ][..],
        &["comments", "add", key.as_str(), "-m", "second", "-f", file],
    ] {
        let refused = machine.run(args.iter().copied().chain(["--json"]));
        assert_eq!(code(&refused), "USAGE", "{args:?}: {}", refused.stderr);
    }
    let shown = machine
        .bead(["comments", key.as_str()])
        .expect("the comments");
    assert_eq!(
        shown,
        beadroll_test_support::Json::Array(Vec::new()),
        "a refused comment was written"
    );
}

/// A title is never cleared: `update --title ""` or spaces alone is `VALIDATION`, as `create`
/// refuses a title that holds nothing, and the title stays; `create -s tombstone` is refused for
/// `bead delete`; `--add-label ""` adds nothing and answers `Unchanged`, writing no event.
#[test]
fn empty_titles_tombstone_creates_and_empty_labels_change_nothing() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "a");
    machine.bead(["init", "--prefix", "ex"]);
    let key = machine.bead(["create", "kept"]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let events = |m: &Machine| m.bead(["info"]).unwrap()["events"].clone();
    let before = events(&machine);
    for title in ["", "   "] {
        let refused = machine.run(["update", key.as_str(), "--title", title, "--json"]);
        assert_eq!(
            code(&refused),
            "VALIDATION",
            "{title:?}: {}",
            refused.stderr
        );
        let created = machine.run(["create", title, "--json"]);
        assert_eq!(
            code(&created),
            "VALIDATION",
            "create {title:?}: {}",
            created.stderr
        );
    }
    let shown = machine.bead(["show", key.as_str()]).unwrap();
    assert_eq!(shown[0]["title"].as_str(), Some("kept"), "{shown:?}");
    let tombstone = machine.run(["create", "gone", "-s", "tombstone", "--json"]);
    assert_eq!(code(&tombstone), "VALIDATION", "{}", tombstone.stderr);
    for label in ["", ","] {
        let unchanged = machine.run(["update", key.as_str(), "--add-label", label]);
        assert_eq!(unchanged.returncode(), 0, "{}", unchanged.stderr);
        assert!(
            unchanged.stdout.contains("Unchanged"),
            "{label:?}: {}",
            unchanged.stdout
        );
    }
    assert_eq!(
        events(&machine),
        before,
        "a refused or empty update wrote an event"
    );
}

/// Taking back a label the issue does not have writes nothing: no `label_remove`, no `updated_at`
/// moved, as a removal affects only the additions its writer saw.
#[test]
fn removing_a_label_the_issue_does_not_have_writes_nothing() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "a");
    machine.bead(["init", "--prefix", "ex"]);
    let key = machine.bead(["create", "x", "-l", "kept"]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let events = |m: &Machine| m.bead(["info"]).unwrap()["events"].clone();
    let before = events(&machine);
    let updated = machine.run(["update", key.as_str(), "--remove-label", "nosuch"]);
    assert_eq!(updated.returncode(), 0, "{}", updated.stderr);
    assert_eq!(events(&machine), before, "a removal of nothing was written");
    machine.bead(["update", key.as_str(), "--remove-label", "kept"]);
    let shown = machine.bead(["show", key.as_str()]).unwrap();
    assert!(
        shown[0].get("labels").is_none_or(|l| l == &json!([])),
        "{shown:?}"
    );
}

/// A missing argument reads as clap means it, `not provided: <ID>...`, never `provided:; <ID>`;
/// `comments add` with nothing after it gives `add`'s own syntax, not a pointer to the `add` typed.
#[test]
fn missing_arguments_say_what_is_missing_and_how_to_give_it() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "a");
    machine.bead(["init", "--prefix", "ex"]);
    let claim = machine.run(["claim", "--json"]);
    assert_eq!(code(&claim), "USAGE", "{}", claim.stderr);
    assert!(
        claim.stderr.contains("not provided: <ID>"),
        "{}",
        claim.stderr
    );
    let add = machine.run(["comments", "add", "--json"]);
    assert_eq!(code(&add), "VALIDATION", "{}", add.stderr);
    assert!(
        add.stderr.contains("usage: bead comments add ISSUE TEXT"),
        "{}",
        add.stderr
    );
    assert!(
        !add.stderr.contains("to add a comment, use"),
        "{}",
        add.stderr
    );
}
