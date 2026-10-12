//! G01 (`docs/FINDINGS.md`): a tracked `remote` starting with `-` reached git as an option, so
//! `--upload-pack=<cmd>` ran a command.

mod g01_tracked_remote_is_never_a_git_option {
    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, git, json};

    /// Spaced, ASCII-escaped JSON.
    fn dumps(value: &Json) -> String {
        write(value, Format::ASCII).expect("a value Python can write")
    }

    /// The id of the issue a `bead create --json` made: `result["id"]`.
    fn id(created: Option<Json>) -> String {
        let created = created.expect("bead printed the issue it made");
        created["id"].as_str().expect("an id").to_owned()
    }

    /// G01: a committed .beadroll.json `remote` beginning with `--upload-pack=` reaches `git fetch`
    /// as an option, so `bead claim` and `bead sync` run the command it names.
    #[test]
    fn tracked_remote_that_looks_like_an_option_runs_nothing() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "g01");
        let claim_marker = sandbox.root().join("RAN-ON-CLAIM");
        let sync_marker = sandbox.root().join("RAN-ON-SYNC");
        let config = a.checkout().join(".beadroll.json");
        let remote = format!("--upload-pack=touch {} ;", claim_marker.display());
        let tracked = json!({"workspace": "ws-g01-0001", "remote": remote});
        std::fs::write(&config, dumps(&tracked)).unwrap();
        a.bead(["init", "--prefix", "ex"]);
        let key = id(a.bead(["create", "work"]));
        let claim = a.run(["claim", &key, "--json"]);
        assert!(
            !claim_marker.exists(),
            "bead claim passed the tracked remote to git as an option and ran the command it named \
             (rc={})",
            claim.returncode()
        );
        let remote = format!("--upload-pack=touch {} ;", sync_marker.display());
        let tracked = json!({"workspace": "ws-g01-0001", "remote": remote});
        std::fs::write(&config, dumps(&tracked)).unwrap();
        let sync = a.run(["sync", "--json"]);
        assert!(
            !sync_marker.exists(),
            "bead sync passed the tracked remote to git as an option and ran the command it named \
             (rc={})",
            sync.returncode()
        );
        assert_ne!(
            sync.returncode(),
            0,
            "an option-shaped remote cannot be a successful coordination remote"
        );

        // The fix must keep honest remotes working, including a relative path that begins with '-'.
        let b = Machine::new(&sandbox, "g01c");
        let bare = b.checkout().join("-coord.git");
        let bare_text = bare.to_str().expect("a UTF-8 path");
        git(["init", "-q", "--bare", bare_text], None);
        let honest_config = json!({"workspace": "ws-g01-0002", "remote": "-coord.git"});
        std::fs::write(b.checkout().join(".beadroll.json"), dumps(&honest_config)).unwrap();
        b.bead(["init", "--prefix", "ex"]);
        let other = id(b.bead(["create", "work"]));
        let honest = b.run(["claim", &other, "--json"]);
        assert_eq!(honest.returncode(), 0, "{}", honest.stderr);
        assert!(
            git(["for-each-ref", "--format=%(refname)"], Some(&bare))
                .contains("refs/beads/journal"),
            "the claim was not published to the dash-named remote"
        );
    }
}
