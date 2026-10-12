//! G24 (`docs/FINDINGS.md`): without .beadroll.json the workspace id is a hash of the origin URL
//! (find_workspace). When the repository is renamed or moved, a machine that updates origin and
//! every fresh clone under the new URL derive another id: the tracker their own origin holds is
//! refused with WORKSPACE_MISMATCH, which names no remedy, and no command adopts it. A fix must
//! still refuse a foreign tracker named with --remote.

mod g24_renamed_repository_loses_its_tracker {
    use std::path::{Path, PathBuf};

    use beadroll_test_support::{Json, Machine, Sandbox, code, git, parse_json};

    /// The path as text passed to git and bead.
    fn text(path: &Path) -> &str {
        path.to_str().expect("a UTF-8 path")
    }

    /// The id of the issue a `--json` result is: `result["id"]`.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        result["id"].as_str().expect("an id").to_owned()
    }

    /// Before the move, machine a publishes its tracker (prefix `rn`), which
    /// holds one issue, `one`; the issue's key.
    fn set_up(sandbox: &Sandbox) -> (Machine, String) {
        let a = Machine::new(sandbox, "a");
        a.bead(["init", "--prefix", "rn"]);
        let issue = id(a.bead(["create", "one"]));
        a.bead(["sync"]);
        (a, issue)
    }

    /// The repository moves, `origin.git` to `renamed.git`, and new clones use
    /// the new URL, which this returns.
    fn move_repository(sandbox: &Sandbox) -> PathBuf {
        let renamed = sandbox.root().join("renamed.git");
        std::fs::rename(sandbox.origin(), &renamed).expect("the repository moves");
        renamed
    }

    /// Reclones the machine's checkout from the moved origin. The machine was initially created
    /// while the sandbox's original remote still existed.
    fn fresh_clone(machine: &Machine, origin: &Path) {
        std::fs::remove_dir_all(machine.checkout()).expect("the machine's first clone");
        git(
            ["clone", "-q", text(origin), text(machine.checkout())],
            None,
        );
    }

    /// G24: after `git remote set-url origin <new URL>`, sync fails with WORKSPACE_MISMATCH against
    /// the machine's own origin and the machine's issues vanish from view.
    #[test]
    fn machine_that_updates_origin_keeps_its_tracker() {
        let sandbox = crate::sandbox();
        let (a, issue) = set_up(&sandbox);
        let renamed = move_repository(&sandbox);
        git(
            ["remote", "set-url", "origin", text(&renamed)],
            Some(a.checkout()),
        );
        let sync = a.run(["sync", "--json"]);
        assert_eq!(sync.returncode(), 0, "{}", sync.stderr);
        assert_eq!(a.show(&issue)["title"].as_str(), Some("one"));
        a.bead(["claim", &issue]);
    }

    /// G24: a fresh clone under the new URL cannot join the tracker its origin holds; a foreign
    /// tracker named with --remote must still be refused, and the refusal must name the way
    /// forward.
    #[test]
    fn fresh_clone_under_the_new_url_uses_the_published_tracker() {
        let sandbox = crate::sandbox();
        let (_, issue) = set_up(&sandbox);
        // made before the move, while test support can still clone `origin.git`
        let (c, d) = (Machine::new(&sandbox, "c"), Machine::new(&sandbox, "d"));
        let renamed = move_repository(&sandbox);
        fresh_clone(&c, &renamed);
        let sync = c.run(["sync", "--json"]);
        assert_eq!(sync.returncode(), 0, "{}", sync.stderr);
        assert_eq!(c.show(&issue)["title"].as_str(), Some("one"));
        // later commands find it without another sync
        let listed = c.bead(["list"]).expect("bead list prints a record");
        assert_eq!(listed["total"].as_i64(), Some(1));
        // guard: adoption is for this checkout's own origin only
        let unrelated = sandbox.clone("unrelated", Some(&sandbox.bare("unrelated.git")));
        fresh_clone(&d, &renamed);
        let foreign = d
            .cmd(["sync", "--remote", text(&renamed), "--json"])
            .cwd(&unrelated)
            .run();
        assert_eq!(code(&foreign), "WORKSPACE_MISMATCH", "{}", foreign.stdout);
        let error = parse_json(&foreign.stderr).expect("a structured error");
        let message = error["error"]["message"].as_str().expect("its message");
        assert!(message.contains("BEAD_WORKSPACE="), "{message}");
        let listed = d.cmd(["list", "--json"]).cwd(&unrelated).run();
        assert_eq!(code(&listed), "NOT_INITIALIZED");
    }
}
