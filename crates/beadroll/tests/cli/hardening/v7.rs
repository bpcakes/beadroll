//! V7: origins with user parts or schemes yield stable workspace ids, and scp-like origins with
//! users sync over SSH. Password-shape and normalization checks are in beadroll-git.

mod v7_origins_with_a_user_or_scheme_are_one_workspace {
    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::OsString;
    use std::fs::Permissions;
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitStatus, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    use beadroll_test_support::{Json, Machine, git, parse_json};

    /// Environment variables for a child process.
    type Env = BTreeMap<OsString, OsString>;

    /// `_v7_git_env`: git with no user, system or global config, no proxies and no prompts: it sees
    /// only `root`. `more` is set last.
    fn git_env(root: &Path, more: &[(&str, &str)]) -> Env {
        let home = root.join("v7-home");
        if !home.is_dir() {
            std::fs::create_dir(&home).unwrap();
        }
        let config = home.join("gitconfig");
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&config)
            .unwrap();
        let unwanted = [
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
            "ssh_askpass",
        ];
        let mut env: Env = std::env::vars_os()
            .filter(|(key, _)| {
                let lower = key.to_string_lossy().to_lowercase();
                !key.as_encoded_bytes().starts_with(b"GIT_") && !unwanted.contains(&lower.as_str())
            })
            .collect();
        env.insert("HOME".into(), home.clone().into());
        env.insert("XDG_CONFIG_HOME".into(), home.into());
        env.insert("GIT_CONFIG_GLOBAL".into(), config.into());
        for (key, value) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("NO_PROXY", "*"),
            ("no_proxy", "*"),
        ]
        .iter()
        .chain(more)
        {
            env.insert(key.into(), value.into());
        }
        env
    }

    /// `_v7_fake_ssh`: an ssh stand-in for `GIT_SSH_COMMAND`: it runs the remote command locally,
    /// in `$V7_SSH_ROOT`.
    fn fake_ssh(root: &Path) -> PathBuf {
        let path = root.join("v7-fake-ssh");
        let script = "#!/bin/sh\n[ \"$1\" = \"-G\" ] && exit 0\nfor last; do :; done\n\
                      PATH=\"$(git --exec-path):$PATH\"\ncd \"$V7_SSH_ROOT\" && exec sh -c \"$last\"\n";
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// `_v7_info`: what `bead info --json` printed, run by `machine` in `cwd` (its checkout when
    /// `None`), which must succeed.
    fn info(machine: &Machine, cwd: Option<&Path>) -> Json {
        let mut invocation = machine.cmd(["info", "--json"]);
        if let Some(cwd) = cwd {
            invocation = invocation.cwd(cwd);
        }
        let proc = invocation.run();
        assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
        parse_json(&proc.stdout).unwrap()
    }

    /// `path` quoted for the shell.
    fn quoted(path: &Path) -> String {
        let text = path.to_str().expect("a UTF-8 path");
        format!("'{}'", text.replace('\'', r"'\''"))
    }

    /// The member `key` of `value`; absence fails the test.
    fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
        value
            .get(key)
            .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
    }

    /// A thread that reads `pipe` to its end, and the channel it hands what it read to.
    fn drain(mut pipe: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            let _ = sender.send(bytes);
        });
        receiver
    }

    /// Runs a command with empty standard input and returns its status and stderr. Kill it and
    /// fail the test if it exceeds `timeout`.
    fn run_within(mut command: Command, timeout: Duration) -> (ExitStatus, String) {
        let deadline = Instant::now() + timeout;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let stdout = drain(child.stdout.take().expect("the standard output"));
        let stderr = drain(child.stderr.take().expect("the standard error"));
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{command:?} had not ended after {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let read = |pipe: &Receiver<Vec<u8>>| {
            let left = deadline.saturating_duration_since(Instant::now());
            let bytes = pipe.recv_timeout(left);
            let bytes = bytes.unwrap_or_else(|_| panic!("{command:?}'s output had not ended"));
            String::from_utf8(bytes).expect("UTF-8 output")
        };
        read(&stdout);
        (status, read(&stderr))
    }

    /// Verified (V7): an origin with a user part or a scheme (scp `git@host.example:org/repo.git`,
    /// `ssh://git@...`, `https://...`, `git://...`) gives one workspace id, and `info` reports the
    /// origin itself as the remote, in a clone, its linked worktree, a subdirectory and a second
    /// clone. The review measured 4 shapes x 4 locations = 16 info calls and 1 distinct id,
    /// 987c37c1f56322a0.
    #[test]
    fn shapes_with_a_user_or_scheme_give_one_workspace_everywhere() {
        let sandbox = crate::sandbox();
        let one = Machine::new(&sandbox, "one");
        let two = Machine::new(&sandbox, "two");
        let worktree = sandbox.root().join("one-wt");
        let added = worktree.to_str().expect("a UTF-8 path");
        git(["worktree", "add", "-q", added], Some(one.checkout()));
        let sub = one.checkout().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let mut ids = BTreeSet::new();
        let mut calls = 0;
        for url in [
            "git@host.example:org/repo.git",
            "ssh://git@host.example/org/repo.git",
            "https://host.example/org/repo.git",
            "git://host.example/org/repo.git",
        ] {
            for machine in [&one, &two] {
                git(
                    ["remote", "set-url", "origin", url],
                    Some(machine.checkout()),
                );
            }
            let places = [
                ("clone", &one, None),
                ("worktree", &one, Some(worktree.as_path())),
                ("subdirectory", &one, Some(sub.as_path())),
                ("second clone", &two, None),
            ];
            for (place, machine, cwd) in places {
                let data = info(machine, cwd);
                calls += 1;
                let workspace = member(&data, "workspace").as_str().expect("an id");
                ids.insert(workspace.to_owned());
                assert_eq!(
                    member(&data, "remote").as_str(),
                    Some(url),
                    "{url} in the {place}"
                );
            }
        }
        assert_eq!(calls, 16);
        assert_eq!(ids, BTreeSet::from(["987c37c1f56322a0".to_owned()]));
    }

    /// Verified (V7): an scp-like origin with a user part (`git@myhost:origin.git`, reached through
    /// an ssh stand-in that runs the remote command in the sandbox) syncs, and the clone, its linked
    /// worktree and a second clone agree on the workspace. The review measured sync rc 0 with pushed
    /// 1 and the one id 606ef04ef5c6313e in all three places.
    #[test]
    fn scp_origin_with_a_user_syncs_over_ssh() {
        let sandbox = crate::sandbox();
        let url = "git@myhost:origin.git";
        let ssh = quoted(&fake_ssh(sandbox.root()));
        let root = sandbox.root().to_str().expect("a UTF-8 path");
        let env = git_env(
            sandbox.root(),
            &[("GIT_SSH_COMMAND", &ssh), ("V7_SSH_ROOT", root)],
        );
        let git_only: Vec<(OsString, OsString)> = env
            .iter()
            .filter(|(key, _)| key.as_encoded_bytes().starts_with(b"GIT_") || *key == "V7_SSH_ROOT")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let mut a = Machine::new(&sandbox, "a");
        let mut b = Machine::new(&sandbox, "b");
        for machine in [&mut a, &mut b] {
            git(
                ["remote", "set-url", "origin", url],
                Some(machine.checkout()),
            );
            machine.extra.extend(git_only.iter().cloned());
        }
        let mut ls_remote = Command::new("git");
        ls_remote
            .args(["ls-remote", "origin"])
            .current_dir(a.checkout())
            .env_clear()
            .envs(&env);
        let (status, stderr) = run_within(ls_remote, Duration::from_secs(60));
        assert!(status.success(), "git itself cannot reach {url}: {stderr}");
        let worktree = sandbox.root().join("a-wt");
        let added = worktree.to_str().expect("a UTF-8 path");
        git(["worktree", "add", "-q", added], Some(a.checkout()));
        // kept local: the sync below is the publish
        let made = a.run(["init", "--prefix", "ex", "--offline", "--json"]);
        assert_eq!(made.returncode(), 0, "{}", made.stderr);
        let synced = a.run(["sync", "--json"]);
        assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
        let result = parse_json(&synced.stdout).unwrap();
        assert_eq!(member(&result, "pushed").as_i64(), Some(1), "{result:?}");
        assert_eq!(member(&result, "remote").as_str(), Some(url));
        let published = git(
            ["for-each-ref", "--format=%(refname)", "refs/beads"],
            Some(sandbox.origin()),
        );
        assert!(
            published
                .split_whitespace()
                .any(|name| name == "refs/beads/journal"),
            "the push did not reach the bare origin"
        );
        let seen = [
            ("clone", info(&a, None)),
            ("worktree", info(&a, Some(&worktree))),
            ("second clone", info(&b, None)),
        ];
        for (place, info) in &seen {
            assert_eq!(
                member(info, "workspace").as_str(),
                Some("606ef04ef5c6313e"),
                "{place}"
            );
            assert_eq!(member(info, "remote").as_str(), Some(url), "{place}");
        }
    }
}
