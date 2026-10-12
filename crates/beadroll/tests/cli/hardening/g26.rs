//! G26 (`docs/FINDINGS.md`): bead creates sync.git with a plain `git init --bare`, so its hash
//! follows this machine's default (init.defaultObjectFormat, GIT_DEFAULT_HASH, Git 3's announced
//! sha256 default), not the project's. A SHA-256 project, or a SHA-256 default against a SHA-1
//! project, can never publish: every push is 'uncertain' (sync: REMOTE_UNREACHABLE 'the push got
//! no answer'; claim: UNRESOLVED), and a sync.git once made in the wrong format stays wrong after
//! the setting is gone.

mod g26_object_format {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use beadroll_test_support::{Json, Machine, Sandbox};

    /// Variables set for a run, over the environment it starts from.
    type Env = BTreeMap<OsString, OsString>;

    /// The path as text passed to git and bead.
    fn text(path: &Path) -> &str {
        path.to_str().expect("a UTF-8 path")
    }

    /// The id of the issue a `--json` result is: `result["id"]`.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        result["id"].as_str().expect("an id").to_owned()
    }

    /// `_g26_config`: a scratch global git config for this sandbox (never the developer's), the
    /// file `name` holding `contents`, as the variables that make git read it and nothing else.
    fn config(sandbox: &Sandbox, name: &str, contents: &str) -> Env {
        let path = sandbox.root().join(name);
        std::fs::write(&path, contents).expect("the scratch config");
        Env::from([
            ("GIT_CONFIG_GLOBAL".into(), path.into_os_string()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ])
    }

    /// `_g26_git`: `git -c user.name=t -c user.email=t@example.invalid ARGS` in `cwd`, with the
    /// test's environment less every `GIT_*` variable, then `env`; what it printed, stripped. A
    /// failure fails the test with git's standard error.
    fn git(args: &[&str], cwd: Option<&Path>, env: &Env) -> String {
        let inherited = std::env::vars_os();
        let kept = inherited.filter(|(key, _)| !key.as_encoded_bytes().starts_with(b"GIT_"));
        let mut command = Command::new("git");
        command
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .env_clear()
            .envs(kept)
            .envs(env)
            .stdin(Stdio::null());
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        let output = command.output().expect("git starts");
        assert!(
            output.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        let printed = String::from_utf8(output.stdout).expect("git's output is UTF-8");
        printed.trim().to_owned()
    }

    /// `machines`: machines a and b, every run of theirs with the variables `genv`.
    fn machines(sandbox: &Sandbox, genv: &Env) -> (Machine, Machine) {
        let (mut a, mut b) = (Machine::new(sandbox, "a"), Machine::new(sandbox, "b"));
        a.extra.extend(genv.clone());
        b.extra.extend(genv.clone());
        (a, b)
    }

    /// `sync_git_format`: the object format of `machine`'s sync.git, as git under the scratch
    /// config `plain` reports it.
    fn sync_git_format(machine: &Machine, plain: &Env) -> String {
        let sync_git = machine.store().join("sync.git");
        let args = [
            "--git-dir",
            text(&sync_git),
            "rev-parse",
            "--show-object-format",
        ];
        git(&args, None, plain)
    }

    /// `assert_published`: from `a_dir`, a's sync and its claim of `issue` succeed, and b, syncing
    /// from `b_dir`, sees the issue in progress; `why` names the case.
    fn assert_published(
        a: &Machine,
        a_dir: &Path,
        b: &Machine,
        b_dir: &Path,
        issue: &str,
        why: &str,
    ) {
        let sync = a.cmd(["sync", "--json"]).cwd(a_dir).run();
        assert_eq!(sync.returncode(), 0, "{why}: {}", sync.stderr);
        let claim = a.cmd(["claim", issue, "--json"]).cwd(a_dir).run();
        assert_eq!(claim.returncode(), 0, "{why}, claim: {}", claim.stderr);
        b.cmd(["sync"]).cwd(b_dir).json();
        let shown = b.cmd(["show", issue]).cwd(b_dir).json();
        let shown = shown.expect("bead show prints a list");
        assert_eq!(
            shown[0]["status"].as_str(),
            Some("in_progress"),
            "{why}: the other machine does not see the claim"
        );
    }

    /// `poison`: what a run under GIT_DEFAULT_HASH=sha256 or a SHA-256 default left: the store's
    /// sync.git in SHA-256.
    fn poison(machine: &Machine, plain: &Env) {
        let path = machine.store().join("sync.git");
        // shutil.rmtree(path, ignore_errors=True)
        let _ = std::fs::remove_dir_all(&path);
        let args = [
            "init",
            "-q",
            "--bare",
            "--object-format=sha256",
            text(&path),
        ];
        git(&args, None, plain);
    }

    /// G26: a SHA-256 repository gets a SHA-1 sync.git and can never publish.
    #[test]
    fn sha256_project_syncs() {
        let sandbox = crate::sandbox();
        let plain = config(&sandbox, "plain.gitconfig", "");
        let origin = sandbox.root().join("origin256.git");
        let init = [
            "init",
            "-q",
            "--bare",
            "--object-format=sha256",
            "-b",
            "main",
            text(&origin),
        ];
        git(&init, None, &plain);
        let (a, b) = machines(&sandbox, &plain);
        // each machine's checkout is a clone of the SHA-256 project (`m.checkout = ...`)
        let (a_dir, b_dir) = (sandbox.root().join("a-256"), sandbox.root().join("b-256"));
        git(&["clone", "-q", text(&origin), text(&a_dir)], None, &plain);
        let start = ["commit", "-q", "--allow-empty", "-m", "start"];
        git(&start, Some(&a_dir), &plain);
        git(&["push", "-q", "origin", "HEAD:main"], Some(&a_dir), &plain);
        git(&["clone", "-q", text(&origin), text(&b_dir)], None, &plain);
        a.cmd(["init", "--prefix", "of"]).cwd(&a_dir).json();
        let issue = id(a.cmd(["create", "one"]).cwd(&a_dir).json());
        assert_published(&a, &a_dir, &b, &b_dir, &issue, "SHA-256 project");
    }

    /// G26: a machine whose default is SHA-256 (init.defaultObjectFormat) cannot publish to a
    /// SHA-1 project, although the user's own git works there.
    #[test]
    fn sha256_default_with_a_sha1_project_syncs() {
        let sandbox = crate::sandbox();
        let sha256 = "[init]\n\tdefaultObjectFormat = sha256\n";
        let (a, b) = machines(&sandbox, &config(&sandbox, "sha256.gitconfig", sha256));
        // the user's own git works here
        git(&["fetch", "-q", "origin"], Some(a.checkout()), &a.extra);
        a.bead(["init", "--prefix", "of"]);
        let issue = id(a.bead(["create", "one"]));
        let why = "SHA-256 default, SHA-1 project";
        assert_published(&a, a.checkout(), &b, b.checkout(), &issue, why);
    }

    /// G26: a SHA-256 sync.git keeps a SHA-1 project's machine from publishing after the setting
    /// that made it is gone, before and after a journal exists on the remote.
    #[test]
    fn sync_git_made_in_the_wrong_format_is_made_again() {
        let sandbox = crate::sandbox();
        let plain = config(&sandbox, "plain.gitconfig", "");
        let (a, b) = machines(&sandbox, &plain);
        a.bead(["init", "--prefix", "of"]);
        let issue = id(a.bead(["create", "one"]));
        poison(&a, &plain);
        let first = a.run(["sync", "--json"]);
        assert_eq!(
            first.returncode(),
            0,
            "no journal published yet: {}",
            first.stderr
        );
        assert_eq!(sync_git_format(&a, &plain), "sha1");
        b.bead(["sync"]);
        poison(&b, &plain);
        a.bead(["create", "two"]);
        a.bead(["sync"]);
        let sync = b.run(["sync", "--json"]);
        assert_eq!(
            sync.returncode(),
            0,
            "against a published journal: {}",
            sync.stderr
        );
        assert_eq!(sync_git_format(&b, &plain), "sha1");
        let why = "after a SHA-256 sync.git was left behind";
        assert_published(&a, a.checkout(), &b, b.checkout(), &issue, why);
    }
}
