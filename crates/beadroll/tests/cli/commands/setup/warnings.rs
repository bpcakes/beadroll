//! The warning `info` and `prime` give while the checkout's `.beadroll.json` does not reach the
//! other checkouts and machines.

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine};

use super::{commit, commit_tracked_file, json, tracked_file};

/// `machine`'s `.beadroll.json` changed in place (`"where": false` added), the change not committed.
fn change_tracked_file(machine: &Machine) {
    let mut changed = tracked_file(machine);
    if let Json::Object(fields) = &mut changed {
        fields.insert("where".into(), Json::Bool(false));
    }
    let body = write(&changed, Format::INDENTED).expect("JSON text");
    std::fs::write(machine.checkout().join(".beadroll.json"), body).expect("the tracked file");
}

/// `info` and `prime` warn while the checkout's `.beadroll.json` does not reach the other checkouts
/// and machines: not committed, only staged, committed but not on origin's default branch, changed
/// after its commit; and say nothing once it is there.
#[test]
fn info_and_prime_warn_about_a_tracked_file_the_others_do_not_see() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    // whatever the user's git hides from its own status
    commit(&a, &["config", "status.showUntrackedFiles", "no"]);
    json(&a, &["init", "--tracked", "--prefix", "ex"]);
    let warning = || -> Option<String> {
        json(&a, &["info"])["tracked_config_warning"]
            .as_str()
            .map(str::to_owned)
    };
    let said = warning().expect("a warning");
    assert!(
        said.starts_with(".beadroll.json is not committed"),
        "{said}"
    );
    let primed = a.run(["prime"]);
    let first = "Warning: .beadroll.json is not committed";
    assert!(primed.stdout.contains(first), "{}", primed.stdout);
    commit(&a, &["add", ".beadroll.json"]);
    let said = warning().expect("a warning");
    assert!(
        said.starts_with(".beadroll.json is added but not committed"),
        "{said}"
    );
    commit(&a, &["commit", "-q", "-m", "tracker"]);
    let said = warning().expect("a warning");
    assert!(said.contains("not on origin's default branch"), "{said}");
    commit(&a, &["push", "-q", "origin", "HEAD:main"]);
    assert_eq!(warning(), None);
    assert!(!a.run(["prime"]).stdout.contains("Warning:"));
    change_tracked_file(&a);
    let said = warning().expect("a warning");
    assert!(
        said.starts_with(".beadroll.json differs from its committed copy"),
        "{said}"
    );
}

/// A checkout with no `refs/remotes/origin/HEAD` (as `git init` and `git remote add` leave it)
/// cannot tell whether origin's default branch has the committed file: `info` says so, and how to
/// let bead check; once origin/HEAD is known, it has nothing to say.
#[test]
fn a_checkout_without_origin_head_says_it_cannot_tell() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    commit(&a, &["remote", "set-head", "origin", "-d"]);
    json(&a, &["init", "--tracked", "--prefix", "ex"]);
    commit_tracked_file(&a);
    let info = json(&a, &["info"]);
    let said = info["tracked_config_warning"].as_str().expect("a warning");
    assert!(
        said.contains("`git remote set-head origin --auto`"),
        "{said}"
    );
    commit(&a, &["remote", "set-head", "origin", "--auto"]);
    assert_eq!(json(&a, &["info"])["tracked_config_warning"].as_str(), None);
}

/// A committed `.beadroll.json` changed under `--skip-worktree`, which hides the change from `git
/// status`: `info` compares the file itself with its commit and still warns.
#[test]
fn a_change_git_status_does_not_show_is_still_warned_about() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--tracked", "--prefix", "ex"]);
    commit_tracked_file(&a);
    assert_eq!(json(&a, &["info"])["tracked_config_warning"].as_str(), None);
    commit(&a, &["update-index", "--skip-worktree", ".beadroll.json"]);
    change_tracked_file(&a);
    let info = json(&a, &["info"]);
    let said = info["tracked_config_warning"].as_str().expect("a warning");
    assert!(
        said.starts_with(".beadroll.json differs from its committed copy"),
        "{said}"
    );
}
