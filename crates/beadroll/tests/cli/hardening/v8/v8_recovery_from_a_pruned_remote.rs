//! A remote that lost its tip commit recovers when its ref is reset to the last valid commit.

use std::ffi::OsStr;

use beadroll_test_support::{Machine, Sandbox, git};

use super::converge_and_compare;

/// `support.git("--git-dir", origin, ARGS)`: what git ARGS printed on the sandbox's remote,
/// stripped.
fn remote_git(sandbox: &Sandbox, args: &[&str]) -> String {
    let words = [OsStr::new("--git-dir"), sandbox.origin().as_os_str()];
    let printed = git(words.into_iter().chain(args.iter().map(OsStr::new)), None);
    printed.trim().to_owned()
}

/// Verified (V8): recovery from a remote whose tip commit was lost (what a concurrent `gc
/// --prune=now` left, ENV-4): `git update-ref refs/beads/journal <last valid commit>` on the bare
/// remote, then syncs, lets all machines converge with no local data lost; the acknowledged claim
/// in the lost commit is republished from the claimant's local copy. The review measured, on its
/// corrupted soak sandbox: exports identical, the same 8 confirmed claims on a, b and c, integrity
/// [] and waiting 0 everywhere, remote fsck clean; nobody could claim during the outage because
/// every fetch failed.
#[test]
fn resetting_the_ref_to_the_last_valid_commit_converges_without_loss() {
    let sandbox = crate::sandbox();
    let [a, b, c] = ["a", "b", "c"].map(|name| Machine::new(&sandbox, name));
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["sync"]);
    let id = |created: Option<beadroll_test_support::Json>| {
        let created = created.expect("an issue");
        created["id"].as_str().expect("an id").to_owned()
    };
    let key = id(a.bead(["create", "work"]));
    let other = id(a.bead(["create", "other"]));
    for machine in [&a, &b, &c] {
        machine.bead(["sync"]);
    }
    let valid = remote_git(&sandbox, &["rev-parse", "refs/beads/journal"]);
    // acknowledged: published in a new tip commit
    c.bead(["claim", &key]);
    let tip = remote_git(&sandbox, &["rev-parse", "refs/beads/journal"]);
    assert_ne!(tip, valid);
    assert_eq!(
        remote_git(&sandbox, &["rev-parse", &format!("{tip}^")]),
        valid
    );
    let loose = sandbox
        .origin()
        .join("objects")
        .join(&tip[..2])
        .join(&tip[2..]);
    // small pushes arrive as loose objects
    assert!(loose.exists(), "{}", loose.display());
    // the lost commit
    std::fs::remove_file(&loose).unwrap();
    let outage = b.run(["claim", &other, "--json"]);
    // nobody can claim while the remote is damaged
    assert_eq!(outage.returncode(), 1, "{}{}", outage.stdout, outage.stderr);
    // the operator's repair
    remote_git(&sandbox, &["update-ref", "refs/beads/journal", &valid]);
    for machine in [&c, &a, &b] {
        machine.bead(["sync"]);
    }
    let claims = converge_and_compare(&[&a, &b, &c], sandbox.origin());
    let confirmed: Vec<&(String, String, String)> = claims["a"]
        .iter()
        .filter(|(_, _, state)| state == "confirmed")
        .collect();
    assert_eq!(confirmed.len(), 1, "{claims:?}");
    assert_eq!(confirmed[0].0, key);
    assert!(confirmed[0].1.contains("c-thread"), "{claims:?}");
    let listed = b.bead(["list", "--all", "--limit", "0"]).expect("a list");
    let issues = listed["issues"].as_array().expect("a list of issues");
    let mut titles: Vec<&str> = issues
        .iter()
        .map(|issue| issue["title"].as_str().expect("a title"))
        .collect();
    titles.sort_unstable();
    assert_eq!(titles, ["other", "work"]);
}
