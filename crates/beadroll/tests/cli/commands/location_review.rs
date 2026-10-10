//! `LocationReview`, from the third independent review: where a claim was made, and the activity
//! that followed it.

use beadroll_test_support::{Machine, git, json};

#[test]
fn claims_and_log_report_claim_location_and_later_activity() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "location-a");
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "located work"]).unwrap();
    let key = created["id"].as_str().unwrap();
    let worktree = sandbox.root().join("located-feature");
    let path = worktree.to_str().unwrap();
    git(
        ["worktree", "add", "-q", path, "-b", "feature/location"],
        Some(a.checkout()),
    );
    std::fs::write(worktree.join("change.txt"), "located\n").unwrap();
    git(["add", "-A"], Some(worktree.as_path()));
    git(
        [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "-m",
            "located change",
        ],
        Some(worktree.as_path()),
    );
    let commit = git(["rev-parse", "HEAD"], Some(worktree.as_path()));
    let commit = commit.trim();

    let claimed = a.cmd(["claim", key]).cwd(&worktree).json().unwrap()[0]["claim"].clone();
    let first = a.bead(["claims"]).unwrap()["claims"][0].clone();
    assert_eq!(
        claimed["where"],
        json!({"branch": "feature/location", "commit": commit})
    );
    assert_eq!(first["branch"], json!("feature/location"));
    a.cmd(["comments", "add", key, "activity after claim"])
        .cwd(&worktree)
        .json();
    let second = a.bead(["claims"]).unwrap()["claims"][0].clone();
    let rows = a.bead(["log", key]).unwrap();
    let later = second["last_activity"].as_str().unwrap();
    let earlier = first["last_activity"].as_str().unwrap();
    assert!(later > earlier, "{later} is not after {earlier}");
    let located: Vec<(Option<&str>, Option<&str>)> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["op"].as_str(), row["branch"].as_str()))
        .collect();
    assert_eq!(
        located,
        [
            (Some("create"), Some("main")),
            (Some("claim"), Some("feature/location")),
            (Some("comment"), Some("feature/location")),
        ]
    );
}
