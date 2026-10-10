//! bead reaches its remote as the user's git would: through the user's ssh command, through a
//! global pushInsteadOf, and through an origin given as a relative path.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use beadroll_test_support::{Machine, git, parse_json};

use super::{Env, git_env, has_ref, scratch_git_config, sh};

/// `path` written for a script, between single quotes as the Python test writes it.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.display())
}

/// Verified (V9): bead's sync honours a GIT_SSH_COMMAND already in the environment (bead only sets
/// its own default when none is present). No real ssh runs: the origin is
/// ssh://fakehost/<sandbox origin>, a fake `ssh` first on PATH and a global core.sshCommand both
/// refuse and log, and the user's wrapper runs the remote git command locally. The review measured
/// the user's git fetch rc 0 and bead sync rc 0, with the wrapper invoked for both fetch
/// (git-upload-pack) and push (git-receive-pack).
#[test]
fn git_ssh_command_in_the_environment_is_honoured() {
    let sandbox = crate::sandbox();
    let bin = sandbox.root().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let log = sandbox.root().join("transport.log");
    std::fs::write(&log, "").unwrap();
    let scripts = [
        (
            "ssh",
            format!(
                "#!/bin/sh\necho \"plain-ssh $*\" >> {}\necho 'plain ssh: blocked in this \
                 test' >&2\nexit 255\n",
                quoted(&log)
            ),
        ),
        (
            "refusing-ssh",
            format!(
                "#!/bin/sh\necho \"core-sshcommand $*\" >> {}\necho 'core.sshCommand: blocked \
                 in this test' >&2\nexit 255\n",
                quoted(&log)
            ),
        ),
        (
            "user-ssh-wrapper",
            format!(
                "#!/bin/sh\necho \"user-ssh-wrapper $*\" >> {}\nfor last; do :; done\nexec sh -c \
                 \"$last\"\n",
                quoted(&log)
            ),
        ),
    ];
    for (name, script) in scripts {
        let file = bin.join(name);
        std::fs::write(&file, script).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let refusing = bin.join("refusing-ssh");
    let genv = scratch_git_config(
        &sandbox,
        &format!("[core]\n\tsshCommand = {}\n", refusing.display()),
    );
    let mut a = Machine::new(&sandbox, "a");
    let url = format!("ssh://fakehost{}", sandbox.origin().display());
    sh(
        &["remote", "set-url", "origin", &url],
        Some(a.checkout()),
        &git_env(&genv),
    );
    let mut path = bin.clone().into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").expect("the test's PATH"));
    a.extra.extend(genv);
    a.extra.insert("PATH".into(), path);
    a.extra.insert(
        "GIT_SSH_COMMAND".into(),
        bin.join("user-ssh-wrapper").into_os_string(),
    );
    let user_fetch = Command::new("git")
        .args(["fetch", "-q", "origin"])
        .current_dir(a.checkout())
        .env_clear()
        .envs(a.env(None, "codex"))
        .stdin(Stdio::null())
        .output()
        .expect("git starts");
    assert!(
        user_fetch.status.success(),
        "{}",
        String::from_utf8_lossy(&user_fetch.stderr)
    );
    std::fs::write(&log, "").unwrap();
    a.bead(["init"]);
    a.bead(["create", "one"]);
    let sync = a.run(["sync", "--json"]);
    assert_eq!(sync.returncode(), 0, "{}", sync.stderr);
    let logged = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = logged.lines().collect();
    let others: Vec<&&str> = lines
        .iter()
        .filter(|line| !line.starts_with("user-ssh-wrapper "))
        .collect();
    assert!(others.is_empty(), "{lines:?}");
    assert!(
        lines.iter().any(|line| line.contains("git-upload-pack")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("git-receive-pack")),
        "{lines:?}"
    );
    assert!(has_ref(sandbox.origin(), "refs/beads/journal"));
}

/// Verified (V9): bead's sync honours a global url.<primary>.pushInsteadOf: origin is a read-only
/// mirror whose pre-receive refuses, and the global config rewrites pushes to it towards the
/// primary. The review measured bead sync rc 0 in 0.4 s; the journal lands on the primary, not the
/// mirror.
#[test]
fn global_push_instead_of_is_honoured() {
    let sandbox = crate::sandbox();
    let ro = sandbox.root().join("ro.git");
    let origin = sandbox.origin().display().to_string();
    git(
        [
            "clone",
            "-q",
            "--mirror",
            &origin,
            &ro.display().to_string(),
        ],
        None,
    );
    let hook = ro.join("hooks").join("pre-receive");
    std::fs::write(&hook, "#!/bin/sh\necho 'read-only mirror' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut a = Machine::new(&sandbox, "a");
    let config = format!("[url \"{origin}\"]\n\tpushInsteadOf = {}\n", ro.display());
    a.extra.extend(scratch_git_config(&sandbox, &config));
    git(
        ["remote", "set-url", "origin", &ro.display().to_string()],
        Some(a.checkout()),
    );
    // one publish: the mirror fetched from never learns of it, a second could not fast-forward
    a.bead(["init", "--offline"]);
    a.bead(["create", "x", "--offline"]);
    let started = Instant::now();
    let sync = a.run(["sync", "--json"]);
    let elapsed = started.elapsed();
    assert_eq!(sync.returncode(), 0, "{}", sync.stderr);
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
    assert!(has_ref(sandbox.origin(), "refs/beads/journal"));
    assert!(!has_ref(&ro, "refs/beads/journal"));
}

/// `fetch_rc`: the status of `git fetch -q origin` in `dir`, under `env`.
fn fetch_rc(dir: &Path, env: &Env) -> i32 {
    let status = Command::new("git")
        .args(["fetch", "-q", "origin"])
        .current_dir(dir)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git starts");
    status.code().expect("git exited")
}

/// `workspace`: the workspace `bead info` resolves in `dir`.
fn workspace(machine: &Machine, dir: &Path) -> String {
    let run = machine.cmd(["info", "--json"]).cwd(dir).run();
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    let info = parse_json(&run.stdout).expect("info prints JSON");
    info["workspace"].as_str().expect("a workspace").to_owned()
}

/// Verified (V9): a relative origin path (../origin.git) resolves to the same workspace from the
/// top of the checkout and from a subdirectory, even when the subdirectory holds an unrelated
/// directory named origin.git. A linked worktree at another directory depth breaks the relative
/// URL for git itself (git fetch rc 128), and bead does not resolve it to the checkout's workspace
/// there either. The review measured top and src/ both 8b1be9c24d984347, worktrees one level
/// deeper 19a8e93ad3f52888, and git fetch rc 128 there.
#[test]
fn relative_origin_resolves_like_git() {
    let sandbox = crate::sandbox();
    let genv = scratch_git_config(&sandbox, "");
    let env = git_env(&genv);
    let mut a = Machine::new(&sandbox, "a");
    a.extra.extend(genv);
    let co = a.checkout().to_path_buf();
    sh(
        &["remote", "set-url", "origin", "../origin.git"],
        Some(&co),
        &env,
    );
    std::fs::create_dir_all(co.join("src").join("origin.git")).unwrap();
    std::fs::create_dir(sandbox.root().join("wts")).unwrap();
    let feature = sandbox.root().join("wts").join("feature");
    let shown = feature.display().to_string();
    sh(
        &["worktree", "add", "-q", &shown, "-b", "feature"],
        Some(&co),
        &env,
    );
    std::fs::create_dir(feature.join("deep")).unwrap();
    let top = workspace(&a, &co);
    assert_eq!(fetch_rc(&co, &env), 0);
    assert_eq!(fetch_rc(&co.join("src"), &env), 0);
    assert_eq!(workspace(&a, &co.join("src")), top);
    for place in [feature.clone(), feature.join("deep")] {
        assert_eq!(fetch_rc(&place, &env), 128, "{}", place.display());
        assert_ne!(workspace(&a, &place), top, "{}", place.display());
    }
}
