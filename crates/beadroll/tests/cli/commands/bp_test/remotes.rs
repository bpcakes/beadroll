//! What reaches the remote: full segments are published once and never rewritten, and a tracked
//! workspace id can coordinate through a remote of its own, which holds nothing but the journal.

use std::collections::BTreeMap;
use std::path::Path;

use beadroll_test_support::{Machine, code, git};

/// Each path the journal ref of the bare repository `remote` holds, with its blob's id.
fn published(remote: &Path) -> BTreeMap<String, String> {
    let remote = remote.to_str().unwrap();
    let ls_tree = ["--git-dir", remote, "ls-tree", "-r", "refs/beads/journal"];
    let rows = git(ls_tree, None);
    let entry = |row: &str| {
        let path = row.split('\t').nth(1).expect("a path after the tab");
        let blob = row.split_whitespace().nth(2).expect("a blob id");
        (path.to_owned(), blob.to_owned())
    };
    rows.lines().map(entry).collect()
}

/// A segment that is full is never rewritten: after more is written and synced, every segment
/// but the last is published with the blob it had, and a new machine reads everything.
#[test]
fn full_segments_are_never_rewritten() {
    let sandbox = crate::sandbox();
    let mut a = Machine::new(&sandbox, "a");
    a.extra = BTreeMap::from([("BEAD_ROTATE_BYTES".into(), "1500".into())]);
    a.bead(["init", "--prefix", "ex"]);
    let body = "x".repeat(300);
    for n in 0..12 {
        let title = format!("issue {n}");
        a.bead(["create", &title, "-d", &body]);
    }
    a.bead(["sync"]);
    let before = published(sandbox.origin());
    // the keys of a BTreeMap are sorted
    let segments: Vec<&String> = before.keys().filter(|p| p.starts_with("events/")).collect();
    assert!(segments.len() > 3, "{segments:?}");
    a.bead(["create", "one more"]);
    a.bead(["sync"]);
    let after = published(sandbox.origin());
    for path in &segments[..segments.len() - 1] {
        assert_eq!(before[*path], after[*path], "{path}");
    }
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    let listed = b.bead(["list", "--limit", "0"]).unwrap();
    assert_eq!(listed["total"].as_i64(), Some(13));
}

/// A workspace id tracked in `.beadroll.json`, with a coordination remote apart from the code's:
/// a repository with another origin joins the tracker by carrying the file; nothing reaches the
/// code remote, and the coordination remote holds the journal and nothing else; a different
/// workspace refuses to adopt its ref.
#[test]
fn tracked_workspace_id_and_a_separate_coordination_remote() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let coordination = sandbox.bare("coordination.git");
    let coordination = coordination.to_str().unwrap();
    let init = [
        "init",
        "--prefix",
        "ex",
        "--tracked",
        "--remote",
        coordination,
    ];
    a.bead(init);
    a.bead(["create", "shared"]);
    a.bead(["sync"]);
    // A repository with a different origin joins the same tracker by carrying the same
    // .beadroll.json.
    let another = sandbox.bare("another-origin.git");
    let elsewhere = sandbox.clone("elsewhere", Some(&another));
    let config = ".beadroll.json";
    std::fs::copy(a.checkout().join(config), elsewhere.join(config)).unwrap();
    let b = Machine::new(&sandbox, "b");
    b.cmd(["sync"]).cwd(&elsewhere).json();
    let listed = b.cmd(["list"]).cwd(&elsewhere).json().unwrap();
    assert_eq!(listed["total"].as_i64(), Some(1));
    let info = b.cmd(["info"]).cwd(&elsewhere).json().unwrap();
    assert_eq!(
        info["workspace_source"].as_str(),
        Some("tracked .beadroll.json")
    );
    // Nothing reached the code remote, and the coordination history holds no code.
    let origin = sandbox.origin().to_str().unwrap();
    assert_eq!(git(["ls-remote", origin, "refs/beads/*"], None), "");
    let names = ["ls-tree", "-r", "--name-only", "refs/beads/journal"];
    let names = git(["--git-dir", coordination].into_iter().chain(names), None);
    let journal_only = names
        .split_whitespace()
        .all(|n| n == "manifest.json" || n.starts_with("events/"));
    assert!(journal_only, "{names}");
    let count = |what: &str| {
        let args = ["--git-dir", coordination, "rev-list", "--count", what];
        git(args, None).trim().to_owned()
    };
    assert_eq!(count("--all"), count("refs/beads/journal"));
    // A different workspace refuses to adopt this ref.
    let c = Machine::new(&sandbox, "c");
    c.bead(["init", "--prefix", "zz"]);
    let refused = c.run(["sync", "--remote", coordination, "--json"]);
    assert_eq!(code(&refused), "WORKSPACE_MISMATCH");
}
