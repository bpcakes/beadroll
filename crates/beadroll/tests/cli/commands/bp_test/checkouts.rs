//! One machine, many checkouts: worktrees and clones share one tracker and stay clean, parallel
//! writers lose nothing, reads need no write access, and each event records where it was made.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use beadroll_core::json::{Format, write};
use beadroll_test_support::events::{Event, id_at};
use beadroll_test_support::{Json, Machine, git, json, parse_json};

use super::{column, id, length, pool_map, segments, sorted_codes};

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
    // The sandbox restores write access when dropped, even if a read fails.
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

/// One `show` of several closed issues says of each whether its closing commit is on the default
/// branch as `show` of each alone says it, the commits asked of git together (roll-lqjj.26): two
/// issues closed on one commit of main and one on another, all pushed; two closed on two commits of
/// a branch never merged; one closed in another clone, on a commit this checkout does not have,
/// which is unknown; an open issue, which has no `merged`; and an issue named twice.
#[test]
fn show_of_several_closed_issues_says_of_each_whether_it_is_merged() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let commit = |cwd: &Path, message: &str| {
        let commit = ["commit", "-q", "--allow-empty", "-m", message];
        git(IDENTITY.into_iter().chain(commit), Some(cwd));
    };
    let closed_in = |cwd: &Path, title: &str| {
        let key = id(&a.bead(["create", title]).unwrap());
        a.cmd(["close", &key]).cwd(cwd).json();
        key
    };
    let main = a.checkout().to_path_buf();
    commit(&main, "one");
    let (m1, m2) = (
        id(&a.bead(["create", "m1"]).unwrap()),
        id(&a.bead(["create", "m2"]).unwrap()),
    );
    a.bead(["close", &m1, &m2]);
    commit(&main, "two");
    let m3 = closed_in(&main, "m3");
    git(["push", "-q", "origin", "main"], Some(&main));
    let wt = sandbox.root().join("wt-unmerged");
    worktree(&main, &wt, "feature/y");
    commit(&wt, "three");
    let f1 = closed_in(&wt, "f1");
    commit(&wt, "four");
    let f2 = closed_in(&wt, "f2");
    let elsewhere = sandbox.clone("a-elsewhere", None);
    commit(&elsewhere, "five");
    let u = closed_in(&elsewhere, "u");
    let open = id(&a.bead(["create", "open"]).unwrap());
    let keys = [&m1, &f1, &u, &m2, &open, &f2, &m3, &f1];
    let shown = a
        .bead(["show"].into_iter().chain(keys.map(String::as_str)))
        .unwrap();
    let merged: Vec<Option<Json>> = (0..keys.len())
        .map(|at| shown[at].get("merged").cloned())
        .collect();
    let (yes, no, unknown) = (Json::Bool(true), Json::Bool(false), Json::from("unknown"));
    assert_eq!(
        merged,
        [
            Some(yes.clone()),
            Some(no.clone()),
            Some(unknown),
            Some(yes.clone()),
            None,
            Some(no.clone()),
            Some(yes),
            Some(no)
        ]
    );
    for (at, key) in keys.iter().enumerate() {
        assert_eq!(
            shown[at],
            a.show(key),
            "{key} shown with the others and alone"
        );
    }
    let together = a.run(["show"].into_iter().chain(keys.map(String::as_str)));
    let alone: String = (keys.iter())
        .map(|key| a.run(["show", key.as_str()]).stdout)
        .collect();
    assert_eq!(together.stdout, alone);
}

/// `merged` of two issues, asked together and checked against each asked alone: one closed on a
/// commit made while the clock was ahead, on a branch that then commits eight times in 2001 and
/// merges into main, where `main_commits` commits were made since it forked; the other closed on a
/// branch never merged. `rev-list ^main` stops walking main's history where its dates fall behind
/// the commit asked, and lists that commit as not on main.
fn merged_with_the_clock_ahead(main_commits: usize) -> Vec<Json> {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let commit_at = |cwd: &Path, message: &str, date: &str| {
        let made = Command::new("git")
            .args(IDENTITY)
            .args(["commit", "-q", "--allow-empty", "-m", message])
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_AUTHOR_DATE", date)
            .current_dir(cwd)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&made.stderr);
        assert!(made.status.success(), "git commit {message}: {stderr}");
    };
    let closed_in = |cwd: &Path, title: &str| {
        let key = id(&a.bead(["create", title]).unwrap());
        a.cmd(["close", &key]).cwd(cwd).json();
        key
    };
    let main = a.checkout().to_path_buf();
    let (ahead, never) = (
        sandbox.root().join("wt-ahead"),
        sandbox.root().join("wt-never"),
    );
    worktree(&main, &ahead, "ahead");
    worktree(&main, &never, "never");
    commit_at(&ahead, "ahead", "2090-01-01T00:00:00Z");
    let x = closed_in(&ahead, "ahead");
    for day in 1..=8 {
        commit_at(&ahead, "behind", &format!("2001-01-0{day}T00:00:00Z"));
    }
    commit_at(&never, "never merged", "2026-01-01T00:00:00Z");
    let y = closed_in(&never, "never merged");
    // main's commits since the branch forked, made at once by fast-import
    let stream: String = (0..main_commits)
        .map(|n| {
            let from = if n == 0 { "from refs/heads/main^0\n" } else { "" };
            let time = 1_767_225_600 + n;
            format!("commit refs/heads/main\ncommitter t <t@example.invalid> {time} +0000\ndata 0\n{from}")
        })
        .collect();
    let mut import = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(&main)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    import
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    assert!(import.wait().unwrap().success(), "git fast-import");
    let merge = ["merge", "-q", "--no-ff", "-m", "merge", "ahead"];
    git(IDENTITY.into_iter().chain(merge), Some(&main));
    let shown = a.bead(["show", &x, &y]).unwrap();
    assert_eq!(shown, Json::Array(vec![a.show(&x), a.show(&y)]));
    (0..2).map(|at| shown[at]["merged"].clone()).collect()
}

/// A commit made while the clock was ahead is merged once main has it, asked together with others
/// as alone: what `rev-list ^main` lists, bead looks for in main's history itself.
#[test]
fn a_commit_made_with_the_clock_ahead_is_merged_once_main_has_it() {
    let merged = merged_with_the_clock_ahead(1);
    assert_eq!(merged, [Json::Bool(true), Json::Bool(false)]);
}

/// So it is where main's history is longer than bead walks, a thousand commits for each commit
/// `rev-list ^main` lists: those commits are asked alone.
#[test]
fn a_commit_made_with_the_clock_ahead_is_merged_however_long_main_is() {
    let merged = merged_with_the_clock_ahead(2_100);
    assert_eq!(merged, [Json::Bool(true), Json::Bool(false)]);
}

/// Closes of the issues `titles` names, each made by another machine (`ffffffff`) on the commit
/// beside its title, as that machine's journal records them: the issues' keys.
fn closed_elsewhere(a: &Machine, titles: &[(&str, &str)]) -> Vec<String> {
    let keys: Vec<String> = (titles.iter())
        .map(|(title, _)| id(&a.bead(["create", title]).unwrap()))
        .collect();
    let mut created = BTreeMap::new();
    for path in segments(&a.store()) {
        for row in std::fs::read_to_string(path).unwrap().lines() {
            let event = parse_json(row).unwrap();
            if let Some(key) = event["data"].get("key").and_then(Json::as_str) {
                created.insert(key.to_owned(), event["id"].as_str().unwrap().to_owned());
            }
        }
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let millis = u64::try_from(now.as_millis()).unwrap() + 1_000;
    let (mut journal, mut previous) = (String::new(), None);
    for (n, (key, (_, commit))) in keys.iter().zip(titles).enumerate() {
        let (event, uid) = (id_at(millis, n as u64), created[key].as_str());
        // each of a machine's events follows the one it wrote before
        let deps: Vec<&str> = [Some(uid), previous.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        let close = Event::new(&event, "close")
            .replica("ffffffff")
            .deps(&deps)
            .issue(uid)
            .with("where", json!({"branch": "main", "commit": (*commit)}));
        journal += &(write(&close.json(), Format::ASCII).unwrap() + "\n");
        previous = Some(event);
    }
    let folder = a.store().join("events").join("ffffffff");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("000001.jsonl"), journal).unwrap();
    keys
}

/// The commit a close names comes from any machine's event, and git reads it as a commit and never
/// as an option: of issues another machine closed on a commit with a NUL in it, on one that begins
/// with `-`, and on this checkout's own commit, `show` says the last is merged and the others are
/// unknown, together as alone (review 16, X14).
#[test]
fn a_closing_commit_from_another_machine_is_read_only_as_a_commit() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let head = git(["rev-parse", "HEAD"], Some(a.checkout()));
    let titles = [
        ("nul", "a\0b"),
        ("option", "--output=x"),
        ("head", head.trim()),
    ];
    let keys = closed_elsewhere(&a, &titles);
    let alone: Vec<Json> = keys.iter().map(|key| a.show(key)).collect();
    let merged: Vec<&Json> = alone.iter().map(|shown| &shown["merged"]).collect();
    let unknown = Json::from("unknown");
    assert_eq!(merged, [&unknown, &unknown, &Json::Bool(true)]);
    let together = a.bead(["show"].into_iter().chain(keys.iter().map(String::as_str)));
    assert_eq!(together, Some(Json::Array(alone)));
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
