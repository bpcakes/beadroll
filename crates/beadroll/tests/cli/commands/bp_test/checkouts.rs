//! One machine, many checkouts: worktrees and clones share one tracker and stay clean, parallel
//! writers lose nothing, reads need no write access, and each event records where it was made.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, git, json, parse_json};

use super::{column, id, length, pool_map, sorted_codes};

/// The identity the sandbox's own commits are made with.
const IDENTITY: [&str; 4] = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];

/// `git worktree add -q PATH -b BRANCH`, run in `checkout`.
fn worktree(checkout: &Path, path: &Path, branch: &str) {
    let path = path.to_str().unwrap();
    git(
        ["worktree", "add", "-q", path, "-b", branch],
        Some(checkout),
    );
}

/// Two worktrees and a second clone of one repository on one machine share its tracker: what is
/// created from each is listed from the others, and bead leaves every checkout clean, with nothing
/// for git to report and no `.beads` directory.
#[test]
fn worktrees_and_clones_share_one_tracker_and_checkouts_stay_clean() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let primary = a.checkout().to_path_buf();
    let (wt1, wt2) = (sandbox.root().join("wt1"), sandbox.root().join("wt2"));
    worktree(&primary, &wt1, "feat-1");
    worktree(&primary, &wt2, "feat-2");
    let clone2 = sandbox.clone("a-second-clone", None);
    a.bead(["init", "--prefix", "ex"]);
    let checkouts = [&primary, &wt1, &wt2, &clone2];
    for (index, cwd) in checkouts.into_iter().enumerate() {
        let title = format!("from checkout {index}");
        a.cmd(["create", &title]).cwd(cwd).json();
    }
    let listed = a.cmd(["list"]).cwd(&wt2).json().unwrap();
    assert_eq!(listed["total"].as_i64(), Some(4));
    let listed = a.cmd(["list"]).cwd(&clone2).json().unwrap();
    assert_eq!(listed["total"].as_i64(), Some(4));
    for cwd in checkouts {
        assert_eq!(git(["status", "--porcelain"], Some(cwd)), "");
        assert!(!cwd.join(".beads").exists(), "{}", cwd.display());
    }
}

/// Thirty creates from two checkouts of one machine, eight at a time, all succeed and lose
/// nothing; of two threads claiming one issue at once, exactly one wins and the other is told it
/// is already claimed.
#[test]
fn parallel_writers_lose_nothing_and_claim_is_exclusive() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let wt = sandbox.root().join("wt");
    worktree(a.checkout(), &wt, "feat");
    a.bead(["init", "--prefix", "ex"]);
    let jobs: Vec<(&Path, String)> = (0..15)
        .flat_map(|n| [a.checkout(), wt.as_path()].map(|cwd| (cwd, format!("job {n}"))))
        .collect();
    let results = pool_map(8, &jobs, |(cwd, title)| {
        a.cmd(["create", title]).cwd(*cwd).run().returncode()
    });
    assert_eq!(results, [0; 30]);
    let listed = a.bead(["list", "--limit", "0"]).unwrap();
    assert_eq!(listed["total"].as_i64(), Some(30));
    let ids: BTreeSet<String> = listed["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(id)
        .collect();
    assert_eq!(ids.len(), 30);
    let target = id(&listed["issues"][0]);
    let claims = pool_map(2, &["thread-one-0001", "thread-two-0002"], |thread| {
        let claim = a.cmd(["update", &target, "--claim", "--json"]);
        claim.thread(*thread).run()
    });
    assert_eq!(sorted_codes(&claims), [0, 1]);
    let stderr: String = claims.iter().map(|c| c.stderr.as_str()).collect();
    assert!(stderr.contains("ALREADY_CLAIMED"), "{stderr}");
}

/// With the machine's home read-only, `show`, `list` and `ready` still work, and a write fails.
#[test]
fn reads_work_when_the_store_is_read_only() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = id(&a.bead(["create", "readable"]).unwrap());
    let status = Command::new("chmod")
        .args(["-R", "a-w"])
        .arg(a.home())
        .status()
        .unwrap();
    assert!(status.success(), "chmod -R a-w {}", a.home().display());
    // The sandbox gives write access back when it drops, as the Python test's `finally` does,
    // however the reads went.
    assert_eq!(a.show(&key)["title"].as_str(), Some("readable"));
    assert_eq!(a.bead(["list"]).unwrap()["total"].as_i64(), Some(1));
    assert_eq!(length(&a.bead(["ready"]).unwrap()), 1);
    assert_ne!(a.run(["create", "nope"]).returncode(), 0);
}

/// Events made in a worktree record its branch, and a close its commit too; asked from the main
/// checkout, which has not merged the branch, the close is not merged, and it is once the branch
/// is merged there. The close ended the claim.
#[test]
fn events_record_the_branch_and_a_close_knows_whether_it_is_merged() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let x = id(&a.bead(["create", "feature work"]).unwrap());
    let wt = sandbox.root().join("wt-feature");
    worktree(a.checkout(), &wt, "feature/x");
    std::fs::write(wt.join("work.txt"), "done\n").unwrap();
    git(["add", "-A"], Some(&wt));
    let commit = ["commit", "-q", "-m", "implement x"];
    git(IDENTITY.into_iter().chain(commit), Some(&wt));
    let head = git(["rev-parse", "HEAD"], Some(&wt)).trim().to_owned();
    a.cmd(["claim", &x]).cwd(&wt).json();
    a.cmd(["comments", "add", &x, "halfway"]).cwd(&wt).json();
    a.cmd(["close", &x, "--reason", "done"]).cwd(&wt).json();
    let shown = a.show(&x); // asked from the main checkout, which has not merged the branch
    assert_eq!(
        shown["closed_on"],
        json!({"branch": "feature/x", "commit": (head.as_str())})
    );
    assert_eq!(shown["comments"][0]["branch"].as_str(), Some("feature/x"));
    assert_eq!(shown["merged"], Json::Bool(false));
    let branches = column(&a.bead(["log", &x]).unwrap(), "branch");
    assert_eq!(branches.last(), Some(&Json::from("feature/x")));
    let merge = ["merge", "-q", "feature/x"];
    git(IDENTITY.into_iter().chain(merge), Some(a.checkout()));
    assert_eq!(a.show(&x)["merged"], Json::Bool(true));
    assert_eq!(
        a.bead(["claims"]).unwrap()["claims"],
        Json::Array(Vec::new())
    );
}

/// An issue created outside any checkout, in the workspace `BEAD_WORKSPACE` names, records no
/// `where`; nor does one created in a checkout whose `.beadroll.json` switches it off.
#[test]
fn where_is_absent_outside_a_checkout_and_when_switched_off() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let store = a.store();
    let store = store.file_name().unwrap().to_str().unwrap();
    let proc = a
        .cmd(["create", "from nowhere", "--json"])
        .cwd(sandbox.root())
        .env("BEAD_WORKSPACE", store)
        .run();
    assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
    let created = id(&parse_json(&proc.stdout).unwrap());
    assert!(a.show(&created).get("where").is_none());
    let config = json!({"workspace": store, "where": false});
    let config = write(&config, Format::ASCII).unwrap();
    std::fs::write(a.checkout().join(".beadroll.json"), config).unwrap();
    let quiet = id(&a.bead(["create", "no branch recorded"]).unwrap());
    assert!(a.show(&quiet).get("where").is_none());
}
