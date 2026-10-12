//! G25: remembering origin's resolved URL must not pin an obsolete remote after `git remote
//! set-url origin`. Both sync and ownership commands must follow the changed URL. Resolution of
//! older stored spellings is checked in beadroll-store's `tests/store/hardening.rs`.

mod g25_first_sync_remote_is_frozen {
    use std::path::Path;

    use beadroll_test_support::{Json, Machine, code, git};

    /// The path as text passed to git and bead.
    fn text(path: &Path) -> &str {
        path.to_str().expect("a UTF-8 path")
    }

    /// The id of the issue a `--json` result is: `result["id"]`.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        result["id"].as_str().expect("an id").to_owned()
    }

    /// `_g25_track`: `init --tracked` (prefix `fz`), committed and pushed, so every clone shares
    /// the workspace id whatever its URL.
    fn track(machine: &Machine) {
        machine.bead(["init", "--prefix", "fz", "--tracked"]);
        let checkout = machine.checkout();
        git(["add", ".beadroll.json"], Some(checkout));
        let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
        let commit = ["commit", "-q", "-m", "track bead"];
        git(identity.into_iter().chain(commit), Some(checkout));
        git(["push", "-q", "origin", "HEAD:main"], Some(checkout));
    }

    /// Reclones the machine's checkout from a different origin.
    fn fresh_clone(machine: &Machine, origin: &Path) {
        std::fs::remove_dir_all(machine.checkout()).expect("the machine's first clone");
        git(
            ["clone", "-q", text(origin), text(machine.checkout())],
            None,
        );
    }

    /// G25: after the repository moves and origin is re-pointed, git fetch works and bead sync and
    /// claim fail against the URL the first sync used.
    #[test]
    fn set_url_is_followed() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "a");
        track(&a);
        let issue = id(a.bead(["create", "one"]));
        a.bead(["sync"]);
        let moved = sandbox.root().join("moved.git");
        std::fs::rename(sandbox.origin(), &moved).expect("the repository moves");
        git(
            ["remote", "set-url", "origin", text(&moved)],
            Some(a.checkout()),
        );
        // git follows the change
        git(["fetch", "-q", "origin"], Some(a.checkout()));
        let sync = a.run(["sync", "--json"]);
        assert_eq!(sync.returncode(), 0, "{}", sync.stderr);
        let info = a.bead(["info"]).expect("bead info prints a record");
        assert_eq!(info["remote"].as_str(), Some(text(&moved)));
        let claim = a.run(["claim", &issue, "--json"]);
        assert_eq!(claim.returncode(), 0, "{}", claim.stderr);
    }

    /// G25: the repository migrates and the old one stays writable. The machine that re-pointed
    /// origin keeps coordinating through the old URL, fresh clones use the new one, and both claims
    /// of one issue are confirmed.
    #[test]
    fn moved_repository_does_not_split_the_crew() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "a");
        track(&a);
        let issue = id(a.bead(["create", "contested"]));
        a.bead(["sync"]);
        let new_home = sandbox.root().join("new-home.git");
        // tracker included
        git(
            [
                "clone",
                "-q",
                "--mirror",
                text(sandbox.origin()),
                text(&new_home),
            ],
            None,
        );
        git(
            ["remote", "set-url", "origin", text(&new_home)],
            Some(a.checkout()),
        );
        // a fresh clone of the new home
        let b = Machine::new(&sandbox, "b");
        fresh_clone(&b, &new_home);
        b.bead(["sync"]);
        b.bead(["claim", &issue]);
        let claim = a.run(["claim", &issue, "--json"]);
        let printed: String = claim.stdout.chars().take(120).collect();
        assert_ne!(
            claim.returncode(),
            0,
            "two exclusive claims of one issue were both confirmed: {printed}"
        );
        assert_eq!(code(&claim), "ALREADY_CLAIMED");
    }
}
