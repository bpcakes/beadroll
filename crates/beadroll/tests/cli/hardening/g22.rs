//! G22 (`docs/FINDINGS.md`): find_workspace takes origin from `git config --get
//! remote.origin.url`, which is the LAST url value and never the pushurl. bead then derives the
//! workspace id from it and coordinates through it, while `git push origin` goes to the pushurl
//! (or the first url) and `git fetch` reads the first url.

mod g22_origin_is_not_where_git_pushes {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use beadroll_test_support::{Json, Machine, Sandbox, git};

    /// `path` as the text git and bead are given, as Python's `str(path)` gives it.
    fn text(path: &Path) -> &str {
        path.to_str().expect("a UTF-8 path")
    }

    /// The id of the issue a `--json` result is: `result["id"]`.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        result["id"].as_str().expect("an id").to_owned()
    }

    /// `_g22_has_journal`: whether the repository `repo` has a `refs/beads/journal`.
    fn has_journal(repo: &Path) -> bool {
        let status = Command::new("git")
            .arg("--git-dir")
            .arg(repo)
            .args(["rev-parse", "-q", "--verify", "refs/beads/journal"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git starts");
        status.success()
    }

    /// `_mirror_machine`: a machine `name` that fetches from a mirror and pushes to the primary
    /// (url=mirror, pushurl=primary), and the mirror, `mirror.git`, cloned from the primary the
    /// first time a test asks for it.
    fn mirror_machine(sandbox: &Sandbox, name: &str) -> (Machine, PathBuf) {
        let mirror = sandbox.root().join("mirror.git");
        let origin = text(sandbox.origin());
        if !mirror.exists() {
            git(["clone", "-q", "--mirror", origin, text(&mirror)], None);
        }
        let m = Machine::new(sandbox, name);
        git(
            ["remote", "set-url", "origin", text(&mirror)],
            Some(m.checkout()),
        );
        git(
            ["remote", "set-url", "--push", "origin", origin],
            Some(m.checkout()),
        );
        let pushes_to = git(
            ["remote", "get-url", "--push", "origin"],
            Some(m.checkout()),
        );
        assert_eq!(pushes_to.trim(), origin);
        (m, mirror)
    }

    /// G22: with remote.origin.pushurl set, bead publishes to the fetch url (a mirror), not where
    /// `git push origin` goes.
    #[test]
    fn pushurl_receives_the_tracker() {
        let sandbox = crate::sandbox();
        let (m, mirror) = mirror_machine(&sandbox, "m");
        m.bead(["init", "--prefix", "pu"]);
        m.bead(["create", "one"]);
        let sync = m.run(["sync", "--json"]);
        assert_eq!(sync.returncode(), 0, "{}", sync.stderr);
        assert!(
            has_journal(sandbox.origin()),
            "the tracker was not published where `git push origin` goes (the pushurl)"
        );
        assert!(
            !has_journal(&mirror),
            "the tracker was published to the fetch-only mirror"
        );
    }

    /// G22: a machine with url=mirror and pushurl=primary derives another workspace id than its
    /// crew's direct clones and cannot join the tracker they publish on the primary.
    #[test]
    fn mirror_fetching_machine_joins_the_crews_tracker() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "a");
        a.bead(["init", "--prefix", "pu"]);
        let issue = id(a.bead(["create", "one"]));
        a.bead(["sync"]);
        let (m, _) = mirror_machine(&sandbox, "m");
        let sync = m.run(["sync", "--json"]);
        assert_eq!(sync.returncode(), 0, "{}", sync.stderr);
        assert_eq!(m.show(&issue)["title"].as_str(), Some("one"));
        let added = id(m.bead(["create", "two"]));
        m.bead(["sync"]);
        a.bead(["sync"]);
        assert_eq!(a.show(&added)["title"].as_str(), Some("two"));
    }

    /// G22: after `git remote set-url --add origin <backup>` (git still fetches from the first url
    /// and pushes code to both), bead coordinates through the last url: NOT_INITIALIZED, and
    /// following its advice would start a second tracker on the backup.
    #[test]
    fn second_url_does_not_move_the_tracker() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "a");
        a.bead(["init", "--prefix", "pu"]);
        let issue = id(a.bead(["create", "one"]));
        a.bead(["sync"]);
        let origin = text(sandbox.origin());
        let backup = sandbox.root().join("backup.git");
        // branches only: no tracker
        git(["clone", "-q", "--bare", origin, text(&backup)], None);
        let c = Machine::new(&sandbox, "c");
        git(
            ["remote", "set-url", "--add", "origin", text(&backup)],
            Some(c.checkout()),
        );
        let fetches_from = git(["remote", "get-url", "origin"], Some(c.checkout()));
        assert_eq!(fetches_from.trim(), origin);
        let info = c.bead(["info"]).expect("bead info prints a record");
        assert_eq!(info["remote"].as_str(), Some(origin));
        let sync = c.run(["sync", "--json"]);
        assert_eq!(sync.returncode(), 0, "{}", sync.stderr);
        assert_eq!(c.show(&issue)["title"].as_str(), Some("one"));
        assert!(!has_journal(&backup));
    }
}
