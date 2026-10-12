//! G23 (`docs/FINDINGS.md`): find_workspace reads .beadroll.json from the working tree only. A
//! checkout whose tree predates the commit that added it (a worktree on an older branch, a bisect,
//! a bare clone) falls back to the URL-derived workspace: NOT_INITIALIZED with 'run `bead init`',
//! and doing so starts a second tracker.

mod g23_tracked_config_read_from_working_tree_only {
    use std::path::Path;

    use beadroll_test_support::{Json, Machine, Sandbox, git, parse_json};

    /// The path as text passed to git and bead.
    fn text(path: &Path) -> &str {
        path.to_str().expect("a UTF-8 path")
    }

    /// The id of the issue a `--json` result is: `result["id"]`.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        result["id"].as_str().expect("an id").to_owned()
    }

    /// `info["workspace"]`, which `bead info` always prints.
    fn workspace(info: Option<Json>) -> Json {
        let info = info.expect("bead info prints a record");
        let workspace = info.get("workspace").cloned();
        workspace.unwrap_or_else(|| panic!("no workspace in {info:?}"))
    }

    /// The class's `setUp`: machine a, whose clone cuts the branch `release-1` and pushes it.
    fn set_up(sandbox: &Sandbox) -> Machine {
        let a = Machine::new(sandbox, "a");
        // cut before bead was tracked
        git(["branch", "release-1"], Some(a.checkout()));
        git(["push", "-q", "origin", "release-1"], Some(a.checkout()));
        a
    }

    /// `_g23_commit_and_push`: `.beadroll.json` committed in `checkout` and pushed to `main`.
    fn commit_and_push(checkout: &Path) {
        git(["add", ".beadroll.json"], Some(checkout));
        let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
        let commit = ["commit", "-q", "-m", "track bead"];
        git(identity.into_iter().chain(commit), Some(checkout));
        git(["push", "-q", "origin", "HEAD:main"], Some(checkout));
    }

    /// G23: a worktree on a branch cut before .beadroll.json was committed, and a bare clone,
    /// resolve to another workspace than the repository's tracked one.
    #[test]
    fn worktree_on_an_older_branch_uses_the_repositorys_tracker() {
        let sandbox = crate::sandbox();
        let a = set_up(&sandbox);
        a.bead(["init", "--tracked"]);
        commit_and_push(a.checkout());
        let issue = id(a.bead(["create", "fix the release build"]));
        a.bead(["sync"]);
        let tracked = workspace(a.bead(["info"]));
        let tree = sandbox.root().join("release-1");
        git(
            ["worktree", "add", "-q", text(&tree), "release-1"],
            Some(a.checkout()),
        );
        assert!(!tree.join(".beadroll.json").exists());
        assert_eq!(
            workspace(a.cmd(["info"]).cwd(&tree).json()),
            tracked,
            "a worktree of the same repository resolves to another workspace"
        );
        let show = a.cmd(["show", &issue, "--json"]).cwd(&tree).run();
        assert_eq!(show.returncode(), 0, "{}", show.stderr);
        let claim = a.cmd(["claim", &issue, "--json"]).cwd(&tree).run();
        assert_eq!(claim.returncode(), 0, "{}", claim.stderr);
        let bare = sandbox.root().join("bare.git");
        git(
            ["clone", "-q", "--bare", text(sandbox.origin()), text(&bare)],
            None,
        );
        assert_eq!(
            workspace(a.cmd(["info"]).cwd(&bare).json()),
            tracked,
            "a bare clone resolves elsewhere"
        );
    }

    /// G23: with `init --tracked --remote <coordination>`, a clone on an older branch also loses
    /// the remote: it is told to run `bead init`, and doing so publishes a second tracker into the
    /// code origin.
    #[test]
    fn older_branch_checkout_keeps_the_tracked_coordination_remote() {
        let sandbox = crate::sandbox();
        let a = set_up(&sandbox);
        let coordination = sandbox.bare("coordination.git");
        a.bead([
            "init",
            "--prefix",
            "tr",
            "--tracked",
            "--remote",
            text(&coordination),
        ]);
        commit_and_push(a.checkout());
        let issue = id(a.bead(["create", "shared issue"]));
        a.bead(["sync"]);
        let c = Machine::new(&sandbox, "c");
        git(["checkout", "-q", "release-1"], Some(c.checkout()));
        let sync = c.run(["sync", "--json"]);
        assert_eq!(sync.returncode(), 0, "{}", sync.stderr);
        let resolved = coordination
            .canonicalize()
            .expect("the coordination remote");
        let result = parse_json(&sync.stdout).expect("sync --json prints JSON");
        assert_eq!(result["remote"].as_str(), Some(text(&resolved)));
        assert_eq!(c.show(&issue)["title"].as_str(), Some("shared issue"));
        let refs = git(
            [
                "--git-dir",
                text(sandbox.origin()),
                "for-each-ref",
                "--format=%(refname)",
                "refs/beads",
            ],
            None,
        );
        assert_eq!(refs, "", "bead published to the code origin");
    }
}
