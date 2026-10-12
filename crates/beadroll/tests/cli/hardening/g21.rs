//! G21: remotes resolve as git resolves them, not against the working directory: a configured
//! remote wins over a directory of its name, and stored relative paths resolve from the top of the
//! checkout (`docs/FINDINGS.md`).

use std::path::Path;
use std::process::Command;

use beadroll_test_support::git;

/// `_g21_nested_repository`: a repository at `path` inside the checkout (vendored code, a
/// submodule-like directory), with one commit on `main`.
fn nested_repository(path: &Path) {
    git(
        [
            "init",
            "-q",
            "-b",
            "main",
            path.to_str().expect("a UTF-8 path"),
        ],
        None,
    );
    let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
    let commit = ["commit", "-q", "--allow-empty", "-m", "v"];
    git(identity.into_iter().chain(commit), Some(path));
}

/// `_g21_beads_refs`: the refs under `refs/beads` of the repository at `path`, as `git for-each-ref`
/// lists them, trimmed; nothing when git lists none, or cannot.
fn beads_refs(path: &Path) -> String {
    let output = Command::new("git")
        .args(["for-each-ref", "refs/beads"])
        .current_dir(path)
        .output()
        .expect("git starts");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

mod g21_remote_resolved_against_the_cwd {
    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, code, git, json, parse_json};

    use super::{beads_refs, nested_repository};

    /// A tracked configuration naming workspace named-ws and remote upstream, as JSON with a
    /// final newline.
    fn named_config() -> String {
        let config = json!({"workspace": "named-ws", "remote": "upstream"});
        write(&config, Format::ASCII).expect("a config") + "\n"
    }

    /// The remote `bead info --json` names, from `info`'s output.
    fn remote(info: &str) -> String {
        let info = parse_json(info).expect("info prints JSON");
        let remote = info.get("remote").and_then(Json::as_str);
        remote
            .unwrap_or_else(|| panic!("no remote in {info:?}"))
            .to_owned()
    }

    /// The key of the issue a `--json` result is.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        let id = result.get("id").and_then(Json::as_str);
        id.unwrap_or_else(|| panic!("no id in {result:?}"))
            .to_owned()
    }

    /// G21: a relative remote path in .beadroll.json is read from the top of the checkout, as git
    /// reads a relative remote URL, so sync works from a subdirectory too.
    #[test]
    fn relative_tracked_remote_works_from_a_subdirectory() {
        let sandbox = crate::sandbox();
        sandbox.bare("tracker.git");
        let machine = Machine::new(&sandbox, "rel");
        std::fs::create_dir(machine.checkout().join("src")).unwrap();
        let made = machine.run([
            "init",
            "--prefix",
            "ex",
            "--tracked",
            "--remote",
            "../tracker.git",
            "--json",
        ]);
        assert_eq!(made.returncode(), 0, "{}", made.stderr);
        let top = machine.run(["sync", "--json"]);
        assert_eq!(top.returncode(), 0, "{}", top.stderr);
        let sub = machine
            .cmd(["sync", "--json"])
            .cwd(machine.checkout().join("src"))
            .run();
        assert_eq!(
            sub.returncode(),
            0,
            "the same tracked remote fails from src/: {}",
            sub.stderr
        );
    }

    /// G21: `init --remote ../../tracker.git` typed in src/ names a tracker beside the checkout;
    /// bead must store it so that sync finds that tracker from the top as well (tracked and
    /// untracked).
    #[test]
    fn relative_remote_given_to_init_in_a_subdirectory_works_from_the_top() {
        let sandbox = crate::sandbox();
        // Separate cases for tracked and untracked workspaces.
        for (tracked, shown) in [(true, "True"), (false, "False")] {
            let tracker = sandbox.bare(&format!("tracker-{shown}.git"));
            let machine = Machine::new(&sandbox, &format!("typed-{shown}"));
            std::fs::create_dir(machine.checkout().join("src")).unwrap();
            let name = tracker
                .file_name()
                .expect("a name")
                .to_str()
                .expect("UTF-8");
            let typed = format!("../../{name}");
            let mut args = vec!["init", "--prefix", "ex", "--remote", &typed, "--json"];
            if tracked {
                args.push("--tracked");
            }
            let made = machine.cmd(args).cwd(machine.checkout().join("src")).run();
            assert_eq!(made.returncode(), 0, "tracked={shown}: {}", made.stderr);
            let info = machine.run(["info", "--json"]);
            assert_eq!(info.returncode(), 0, "tracked={shown}: {}", info.stderr);
            assert_eq!(
                remote(&info.stdout),
                tracker.to_str().expect("a UTF-8 path"),
                "tracked={shown}"
            );
            let top = machine.run(["sync", "--json"]);
            assert_eq!(
                top.returncode(),
                0,
                "tracked={shown}: the remote given in src/ is lost at the top: {}",
                top.stderr
            );
        }
    }

    /// G21: a configured remote whose URL is relative (../tracker.git) is reached from a
    /// subdirectory, as git reaches it (`git ls-remote tr` works there).
    #[test]
    fn named_remote_with_a_relative_url_works_from_a_subdirectory() {
        let sandbox = crate::sandbox();
        sandbox.bare("tracker.git");
        let machine = Machine::new(&sandbox, "named-rel");
        let src = machine.checkout().join("src");
        std::fs::create_dir(&src).unwrap();
        let add = ["remote", "add", "tr", "../tracker.git"];
        git(add, Some(machine.checkout()));
        // git itself reaches it from src/
        git(["ls-remote", "tr"], Some(&src));
        let init = machine.run(["init", "--prefix", "ex"]);
        assert_eq!(init.returncode(), 0, "{}", init.stderr);
        let sub = machine
            .cmd(["sync", "--remote", "tr", "--json"])
            .cwd(&src)
            .run();
        assert_eq!(sub.returncode(), 0, "{}", sub.stderr);
    }

    /// G21: like git, a configured remote name is that remote even when the cwd holds a directory
    /// of the same name; bead must not publish the journal into a repository inside the checkout.
    #[test]
    fn configured_remote_name_wins_over_a_same_named_directory() {
        let sandbox = crate::sandbox();
        let machine = Machine::new(&sandbox, "shadow");
        let team = sandbox.bare("team.git");
        let team_path = team.to_str().expect("a UTF-8 path");
        git(
            ["remote", "add", "upstream", team_path],
            Some(machine.checkout()),
        );
        let config = machine.checkout().join(".beadroll.json");
        std::fs::write(config, named_config()).unwrap();
        let vendored = machine.checkout().join("upstream");
        nested_repository(&vendored);
        let init = machine.run(["init", "--prefix", "ex"]);
        assert_eq!(init.returncode(), 0, "{}", init.stderr);
        let info = machine.run(["info", "--json"]);
        assert_eq!(info.returncode(), 0, "{}", info.stderr);
        assert_eq!(remote(&info.stdout), team_path);
        let synced = machine.run(["sync", "--json"]);
        assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
        assert_eq!(
            beads_refs(&vendored),
            "",
            "the journal was pushed into a repository inside the checkout"
        );
        let published = beads_refs(&team);
        assert!(
            published.contains("refs/beads/journal"),
            "no journal in {published:?}"
        );
    }

    /// G21: with the remote shadowed at the top of one checkout, that machine's claim lands in the
    /// nested repository and a second machine is also granted the claim against the real remote.
    #[test]
    fn a_directory_named_like_the_remote_does_not_let_two_machines_claim_one_issue() {
        let sandbox = crate::sandbox();
        let team = sandbox.bare("team.git");
        let team_path = team.to_str().expect("a UTF-8 path");
        let config = named_config();
        let a = Machine::new(&sandbox, "a");
        let b = Machine::new(&sandbox, "b");
        for machine in [&a, &b] {
            git(
                ["remote", "add", "upstream", team_path],
                Some(machine.checkout()),
            );
            std::fs::write(machine.checkout().join(".beadroll.json"), &config).unwrap();
        }
        nested_repository(&a.checkout().join("upstream"));
        let src = a.checkout().join("src");
        std::fs::create_dir(&src).unwrap();
        let init = a.cmd(["init", "--prefix", "ex"]).cwd(&src).run();
        assert_eq!(init.returncode(), 0, "{}", init.stderr);
        let key = id(a.cmd(["create", "shared work"]).cwd(&src).json());
        a.cmd(["sync"]).cwd(&src).json();
        let init = b.run(["init", "--prefix", "ex"]);
        assert_eq!(init.returncode(), 0, "{}", init.stderr);
        b.bead(["sync"]);
        // at the top of A's checkout
        let first = a.run(["claim", &key, "--json"]);
        assert_eq!(first.returncode(), 0, "{}", first.stderr);
        let second = b.run(["claim", &key, "--json"]);
        let shown: String = second.stdout.trim().chars().take(200).collect();
        assert_ne!(
            second.returncode(),
            0,
            "B was granted a claim A already holds: {shown}"
        );
        assert_eq!(code(&second), "ALREADY_CLAIMED", "{}", second.stderr);
    }
}
