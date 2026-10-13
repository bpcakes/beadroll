//! `--dry-run` of `bead init --import FILE` and `bead import FILE` (roll-dih8.26): what the import
//! would do, every problem it would be refused for, and where the Beads tracker has changes the
//! checkout lacks; nothing imported, no store started, no remote asked.

use std::path::Path;

use beadroll_test_support::{Json, Machine, code, git};

/// A Beads export: ex-1 open, ex-2 in progress and waiting for ex-9, which it does not hold,
/// ex-3 closed.
const EXPORT: &str = concat!(
    "{\"id\":\"ex-1\",\"title\":\"one\",\"status\":\"open\"}\n",
    "{\"id\":\"ex-2\",\"title\":\"two\",\"status\":\"in_progress\",\"dependencies\":",
    "[{\"issue_id\":\"ex-2\",\"depends_on_id\":\"ex-9\",\"type\":\"blocks\"}]}\n",
    "{\"id\":\"ex-3\",\"title\":\"three\",\"status\":\"closed\",",
    "\"closed_at\":\"2026-01-02T00:00:00Z\"}\n",
);

/// The tracker file `machine`'s checkout keeps, as Beads keeps it, holding `text`.
fn beads_file(machine: &Machine, text: &str) -> String {
    let dir = machine.checkout().join(".beads");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("issues.jsonl"), text).unwrap();
    ".beads/issues.jsonl".to_owned()
}

/// Whether the remote publishes a tracker.
fn published(origin: &Path) -> bool {
    !git(
        ["ls-remote", origin.to_str().unwrap(), "refs/beads/journal"],
        None,
    )
    .is_empty()
}

/// `bead ARGS --json` on `machine`, which must succeed: its answer.
fn answer(machine: &Machine, args: &[&str]) -> Json {
    let run = machine.cmd(args.iter().chain(&["--json"])).run();
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    beadroll_test_support::parse_json(&run.stdout).unwrap()
}

#[test]
fn init_says_what_it_would_import_and_writes_nothing() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let file = beads_file(&a, EXPORT);
    let said = answer(&a, &["init", "--import", &file, "--dry-run"]);
    assert_eq!(said["importable"], Json::Bool(true));
    assert_eq!(said["new"].as_i64(), Some(3));
    assert_eq!(said["known"].as_i64(), Some(0));
    assert_eq!(said["statuses"]["in_progress"].as_i64(), Some(1));
    assert_eq!(said["unowned"][0].as_str(), Some("ex-2"));
    assert_eq!(said["waiting"][0].as_str(), Some("ex-9"));
    // nothing started here (bead makes a machine's home, `home-NAME`, with its first store) or
    // there
    assert!(!sandbox.root().join("home-a").exists());
    assert!(!published(sandbox.origin()));
    let human = a.run(["init", "--import", &file, "--dry-run"]);
    assert!(
        human
            .stdout
            .contains("Would import 3 issues from .beads/issues.jsonl"),
        "{}",
        human.stdout
    );
    assert!(
        human.stdout.contains("The remote was not asked"),
        "{}",
        human.stdout
    );
    // and the import does what it said
    let done = a.bead(["init", "--import", &file]).unwrap();
    assert_eq!(done["imported"].as_i64(), Some(3));
    assert!(published(sandbox.origin()));
}

#[test]
fn every_problem_is_listed_and_the_dry_run_still_answers() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    // two Beads memory records, which have no id, and a repeated id
    let text = format!(
        "{EXPORT}{{\"_type\":\"memory\",\"key\":\"a\",\"value\":\"x\"}}\n\
         {{\"_type\":\"memory\",\"key\":\"b\",\"value\":\"y\"}}\n{{\"id\":\"ex-1\",\"title\":\"again\"}}\n"
    );
    let file = beads_file(&a, &text);
    let said = answer(&a, &["init", "--import", &file, "--dry-run"]);
    assert_eq!(said["importable"], Json::Bool(false));
    let problems: Vec<&str> = (said["problems"].as_array().unwrap().iter())
        .map(|p| p.as_str().unwrap())
        .collect();
    assert_eq!(
        problems,
        [
            ".beads/issues.jsonl:4: record has no string 'id'",
            ".beads/issues.jsonl:5: record has no string 'id'",
            ".beads/issues.jsonl:6: record id 'ex-1' repeats record 1",
        ]
    );
    let human = a.run(["init", "--import", &file, "--dry-run"]);
    assert_eq!(human.returncode(), 0, "{}", human.stderr);
    assert!(
        human
            .stdout
            .contains("Would refuse .beads/issues.jsonl: 3 problems"),
        "{}",
        human.stdout
    );
    // the import refuses it as the dry run said, at the first problem
    let refused = a.run(["init", "--import", &file, "--json"]);
    assert_eq!(code(&refused), "VALIDATION", "{}", refused.stderr);
}

#[test]
fn branches_and_worktrees_with_tracker_changes_the_checkout_lacks_are_named() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let checkout = a.checkout();
    let file = beads_file(&a, EXPORT);
    let at = Some(checkout);
    git(["add", ".beads"], at);
    git(["commit", "-q", "-m", "tracker"], at);
    // behind HEAD: nothing it lacks
    git(["branch", "behind", "HEAD~1"], at);
    // ahead: one commit that touches the tracker file, one that does not
    git(["checkout", "-q", "-b", "ahead"], at);
    let more = format!("{EXPORT}{{\"id\":\"ex-4\",\"title\":\"four\"}}\n");
    beads_file(&a, &more);
    git(["commit", "-q", "-am", "tracker work"], at);
    git(["commit", "-q", "--allow-empty", "-m", "code work"], at);
    git(["checkout", "-q", "-"], at);
    // a worktree with tracker changes not committed, and one with none
    let changed = checkout.with_file_name("changed-tree");
    let clean = checkout.with_file_name("clean-tree");
    for (tree, branch) in [(&changed, "changed"), (&clean, "clean")] {
        git(
            [
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                tree.to_str().unwrap(),
            ],
            at,
        );
    }
    std::fs::write(changed.join(".beads/issues.jsonl"), &more).unwrap();
    let said = answer(&a, &["init", "--import", &file, "--dry-run"]);
    let refs = said["elsewhere"]["refs"].as_array().unwrap();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0]["ref"].as_str(), Some("refs/heads/ahead"));
    assert_eq!(refs[0]["commits"].as_i64(), Some(1));
    let trees = said["elsewhere"]["worktrees"].as_array().unwrap();
    assert_eq!(trees.len(), 1, "{trees:?}");
    assert!(
        trees[0].as_str().unwrap().ends_with("changed-tree"),
        "{trees:?}"
    );
    let human = a.run(["init", "--import", &file, "--dry-run"]);
    assert!(
        human
            .stdout
            .contains("refs/heads/ahead: 1 commit touching it that HEAD has not"),
        "{}",
        human.stdout
    );
}

#[test]
fn import_plans_against_the_tracker_here() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let file = beads_file(&a, EXPORT);
    a.bead(["init", "--import", &file]);
    let events = a.bead(["log"]).unwrap().as_array().unwrap().len();
    let more = format!("{EXPORT}{{\"id\":\"ex-4\",\"title\":\"four\"}}\n");
    let file = beads_file(&a, &more);
    let said = answer(&a, &["import", &file, "--dry-run"]);
    assert_eq!(said["importable"], Json::Bool(true));
    assert_eq!(
        (said["new"].as_i64(), said["known"].as_i64()),
        (Some(1), Some(3))
    );
    assert_eq!(a.bead(["log"]).unwrap().as_array().unwrap().len(), events);
}

#[test]
fn init_dry_run_needs_an_import() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let refused = a.run(["init", "--dry-run", "--json"]);
    assert_eq!(refused.returncode(), 2, "{}", refused.stderr);
    assert_eq!(code(&refused), "USAGE");
    assert!(!sandbox.root().join("home-a").exists());
}
