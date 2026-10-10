//! G41: a remote whose journal ref names objects it no longer has is damaged, not unreachable.

mod g41_damaged_remote_is_not_an_unreachable_one {
    use beadroll_test_support::{Machine, git, parse_json};

    /// G41: when the remote's refs/beads/journal names a commit or tree the remote no longer has
    /// (what a `git gc --prune=now` racing a push leaves), every fetch fails and bead reports
    /// REMOTE_UNREACHABLE 'cannot reach the coordination remote', which PRIME defines as 'nothing
    /// is lost; retry later'. The remote answered, no retry can succeed, and the message never
    /// names the damaged ref.
    #[test]
    fn remote_ref_naming_lost_objects_is_reported_as_damage_naming_the_ref() {
        for lost in ["commit", "tree"] {
            let sandbox = crate::sandbox();
            let a = Machine::new(&sandbox, &format!("g41a-{lost}"));
            let b = Machine::new(&sandbox, &format!("g41b-{lost}"));
            a.bead(["init", "--prefix", "ex"]);
            a.bead(["sync"]);
            b.bead(["sync"]);
            let created = a.bead(["create", "work"]).expect("create prints the issue");
            let key = created["id"].as_str().expect("the issue's id").to_owned();
            a.bead(["sync"]); // b has not fetched this commit
            let spec = match lost {
                "commit" => "refs/beads/journal",
                _ => "refs/beads/journal^{tree}",
            };
            let origin = sandbox.origin().to_str().expect("a UTF-8 path");
            let sha = git(["--git-dir", origin, "rev-parse", spec], None);
            let sha = sha.trim();
            let loose = sandbox
                .origin()
                .join("objects")
                .join(&sha[..2])
                .join(&sha[2..]);
            assert!(loose.exists(), "a small push arrives as loose objects");
            // what a concurrent `git gc --prune=now` did to a just-pushed object in this review's
            // soak
            std::fs::remove_file(&loose).expect("the loose object");
            let commands: [&[&str]; 2] = [&["sync"], &["claim", &key]];
            for argv in commands {
                let proc = b.run(argv.iter().chain(&["--json"]));
                assert_eq!(proc.returncode(), 1, "{lost}: {}", proc.stdout);
                let document = parse_json(&proc.stderr)
                    .unwrap_or_else(|error| panic!("{lost}: {error}: {}", proc.stderr));
                let error = document.get("error");
                let error = error.unwrap_or_else(|| panic!("{lost}: no error in {}", proc.stderr));
                let code = error.get("code").expect("the error's code");
                let message = error.get("message").and_then(|m| m.as_str());
                let message = message.unwrap_or_else(|| panic!("no message in {}", proc.stderr));
                assert_ne!(
                    code.as_str(),
                    Some("REMOTE_UNREACHABLE"),
                    "{lost}: {}: a remote that answered is reported unreachable ('nothing is \
                     lost; retry later'): {message}",
                    argv[0]
                );
                assert!(
                    message.contains("refs/beads/journal"),
                    "{lost}: {} does not name the damaged ref: {message}",
                    argv[0]
                );
            }
        }
    }
}
