//! Dependency edges held typed, each addition's metadata kept with its issue's texts
//! (roll-lqjj.39): bead answers alike from its projection, its texts read in where a command needs
//! them, and from the journal replayed, for edges of every kind, an import's metadata with members
//! of its own, an edge an awaited key binds after the projection was written, and edges removed and
//! added again.

use super::{answers, projection};

/// The reads that answer from edges beside those `answers` makes: the children of a parent, a
/// subtree's readiness, a tree of dependencies and the cycles.
fn edge_reads(machine: &beadroll_test_support::Machine) -> Vec<String> {
    let reads: [&[&str]; 6] = [
        &["list", "--parent", "ex-e", "--json"],
        &["ready", "ex-e", "--json"],
        &["dep", "tree", "ex-d"],
        &["dep", "list", "ex-c", "--json"],
        &["dep", "cycles", "--json"],
        &["show", "ex-c", "ex-d", "ex-e"],
    ];
    (reads.iter())
        .map(|args| {
            let run = machine.run(*args);
            assert_eq!(run.returncode(), 0, "{args:?}: {}", run.stderr);
            run.stdout
        })
        .collect()
}

#[test]
fn edges_and_their_metadata_answer_alike_from_the_texts_and_from_the_journal() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "kept-a");
    a.bead(["init", "--prefix", "ex"]);
    let first = a.home().join("first.jsonl");
    let records = [
        r#"{"id": "ex-e", "title": "epic", "status": "open", "priority": 1, "issue_type": "epic"}"#,
        r#"{"id": "ex-c", "title": "child", "status": "open", "priority": 2, "issue_type": "task", "dependencies": [{"issue_id": "ex-c", "depends_on_id": "ex-e", "type": "parent-child", "created_at": "2026-01-01T00:00:00Z", "created_by": "seed", "metadata": "{\"why\":\"split\",\"weight\":3}", "thread_id": "th-1"}, {"issue_id": "ex-c", "depends_on_id": "ex-far", "type": "related", "created_at": "2026-01-02T00:00:00Z", "created_by": "importer", "metadata": "{\"url\":\"https://example.invalid\"}", "thread_id": "th-2"}]}"#,
        r#"{"id": "ex-d", "title": "found", "status": "open", "priority": 2, "issue_type": "bug", "dependencies": [{"issue_id": "ex-d", "depends_on_id": "ex-c", "type": "discovered-from", "created_at": "2026-01-03T00:00:00Z", "created_by": "seed"}, {"issue_id": "ex-d", "depends_on_id": "ex-c", "type": "tracks", "metadata": "{}", "thread_id": ""}]}"#,
    ];
    std::fs::write(&first, records.join("\n") + "\n").unwrap();
    a.bead(["import", first.to_str().unwrap()]);
    let keys: Vec<String> = ["ex-e", "ex-c", "ex-d"].map(str::to_owned).to_vec();
    let _ = answers(&a, &keys, false);
    assert!(projection(&a).is_file(), "a read keeps a projection");
    // the key ex-c waits for arrives after the projection was written: its edge is made in ex-c,
    // whose texts are only in the text file
    let second = a.home().join("second.jsonl");
    let far = r#"{"id": "ex-far", "title": "far", "status": "open", "priority": 3, "issue_type": "task"}"#;
    std::fs::write(&second, format!("{far}\n")).unwrap();
    a.bead(["import", second.to_str().unwrap()]);
    a.bead(["dep", "add", "ex-d", "ex-e", "--type", "blocks"]);
    a.bead(["dep", "add", "ex-e", "ex-d", "--type", "related"]);
    a.bead(["dep", "remove", "ex-d", "ex-c", "--type", "tracks"]);
    a.bead(["dep", "add", "ex-d", "ex-c", "--type", "tracks"]);
    let kept = (answers(&a, &keys, false), edge_reads(&a));
    let _ = std::fs::remove_file(projection(&a));
    let replayed = (answers(&a, &keys, true), edge_reads(&a));
    assert_eq!(kept, replayed);
    let export = &kept.0[4];
    for member in [
        r#""metadata":"{\"why\":\"split\",\"weight\":3}""#,
        r#""metadata":"{\"url\":\"https://example.invalid\"}""#,
        r#""thread_id":"th-2""#,
        r#""depends_on_id":"ex-far""#,
    ] {
        assert!(export.contains(member), "{member} in {export}");
    }
}
