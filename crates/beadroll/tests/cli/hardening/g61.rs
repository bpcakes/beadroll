//! G61: whether an issue's closing commit is merged is judged against origin's default branch,
//! whatever the local one says and whatever slashes its name holds.

mod g61_merged_check_sees_origin_main {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    use beadroll_test_support::{Json, Machine, Sandbox};
    use rustix::process::{Pid, Signal, kill_process};

    /// A scratch global git configuration, and no system one.
    fn genv(sandbox: &Sandbox) -> [(OsString, OsString); 2] {
        let config = sandbox.root().join("global.gitconfig");
        [
            ("GIT_CONFIG_GLOBAL".into(), config.into_os_string()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ]
    }

    /// Machine `a`, under the scratch git configuration, with a tracker (prefix `mg`).
    fn set_up(sandbox: &Sandbox) -> Machine {
        std::fs::write(sandbox.root().join("global.gitconfig"), "").unwrap();
        let mut a = Machine::new(sandbox, "a");
        a.extra.extend(genv(sandbox));
        a.bead(["init", "--prefix", "mg"]);
        a
    }

    /// `git ARGS` in `cwd` as author and committer `t`, with the test's
    /// environment less every `GIT_*` variable, plus the scratch configuration; what it printed,
    /// stripped. A failure fails the test, and so does git still running after 30 seconds.
    fn git(sandbox: &Sandbox, args: &[&str], cwd: &Path) -> String {
        let inherited = std::env::vars_os().filter(|(key, _)| {
            let key = key.as_encoded_bytes();
            !key.starts_with(b"GIT_")
        });
        let child = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(inherited)
            .envs(genv(sandbox))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("git starts");
        let pid = Pid::from_child(&child);
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || sender.send(child.wait_with_output()));
        let Ok(output) = receiver.recv_timeout(Duration::from_secs(30)) else {
            let _ = kill_process(pid, Signal::KILL);
            panic!("git {} took more than 30 seconds", args.join(" "));
        };
        let output = output.expect("git's output");
        assert!(
            output.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        let printed = String::from_utf8(output.stdout).expect("git's output is UTF-8");
        printed.trim().to_owned()
    }

    /// `close_on_a_feature_branch`: an issue closed from a worktree on branch `feat`, whose commit
    /// is not on any default branch yet; its key and the worktree.
    fn close_on_a_feature_branch(sandbox: &Sandbox, a: &Machine) -> (String, PathBuf) {
        let created = a.bead(["create", "one"]).expect("bead printed a result");
        let key = created["id"].as_str().expect("an id").to_owned();
        let tree = sandbox.root().join("wt");
        let worktree = tree.to_str().unwrap();
        let add = ["worktree", "add", "-q", worktree, "-b", "feat"];
        git(sandbox, &add, a.checkout());
        std::fs::write(tree.join("f"), "x").unwrap();
        git(sandbox, &["add", "f"], &tree);
        git(sandbox, &["commit", "-q", "-m", "feat"], &tree);
        a.cmd(["close", &key, "-r", "done"]).cwd(&tree).json();
        (key, tree)
    }

    /// `show KEY`'s `merged`, which must be there: `assertIs` compares it with `True` or `False`.
    fn merged(a: &Machine, key: &str) -> Json {
        a.show(key).get("merged").expect("show's merged").clone()
    }

    /// G61: `show` says the closing commit is merged into the default branch when it is on
    /// origin/main, although the local main (usually stale in a worktree workflow) has not been
    /// updated: the local branch is no longer checked first and left to decide alone.
    #[test]
    fn commit_on_origin_main_is_merged_although_local_main_is_stale() {
        let sandbox = crate::sandbox();
        let a = set_up(&sandbox);
        let (key, tree) = close_on_a_feature_branch(&sandbox, &a);
        // control: not merged anywhere yet
        assert_eq!(merged(&a, &key), Json::Bool(false));
        // merged on the server
        git(&sandbox, &["push", "-q", "origin", "feat:main"], &tree);
        git(&sandbox, &["fetch", "-q", "origin"], a.checkout());
        let shown = a.show(&key);
        let closed_on = shown.get("closed_on").expect("show's closed_on");
        let closing = closed_on.get("commit").expect("the closing commit");
        let closing = closing.as_str().expect("a commit");
        let ancestor = [
            "merge-base",
            "--is-ancestor",
            closing,
            "refs/remotes/origin/main",
        ];
        git(&sandbox, &ancestor, a.checkout());
        assert_ne!(
            git(&sandbox, &["rev-parse", "refs/heads/main"], a.checkout()),
            git(
                &sandbox,
                &["rev-parse", "refs/remotes/origin/main"],
                a.checkout()
            )
        );
        let human = a.run(["show", &key]);
        assert_eq!(human.returncode(), 0, "{}", human.stderr);
        assert_eq!(
            merged(&a, &key),
            Json::Bool(true),
            "a closing commit already on origin/main is reported as not merged: {}",
            human.stdout
        );
        assert!(
            human.stdout.contains("merged into the default branch"),
            "{}",
            human.stdout
        );
    }

    /// G61: a default branch named `release/main` (origin/HEAD) is looked up whole, not as `main`,
    /// so a closing commit merged into origin/release/main is reported as on the default branch.
    #[test]
    fn default_branch_with_a_slash_is_looked_up_whole() {
        let sandbox = crate::sandbox();
        let a = set_up(&sandbox);
        let (key, tree) = close_on_a_feature_branch(&sandbox, &a);
        let push = ["push", "-q", "origin", "feat:refs/heads/release/main"];
        git(&sandbox, &push, &tree);
        git(&sandbox, &["fetch", "-q", "origin"], a.checkout());
        let head = [
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/release/main",
        ];
        git(&sandbox, &head, a.checkout());
        assert_eq!(
            merged(&a, &key),
            Json::Bool(true),
            "a closing commit on origin/release/main (the default branch) is reported as not merged"
        );
    }
}
