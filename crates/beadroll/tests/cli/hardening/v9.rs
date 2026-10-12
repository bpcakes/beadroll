//! V9: bead among the user's git: global settings, checkout hooks, the git environment a hook or an
//! alias runs in, every shape of checkout, and the transport and URL settings git honours. The
//! shared helpers are here, with test scenarios grouped in the modules below.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, Stdio};

use beadroll_test_support::Sandbox;

mod v9_caller_git_environment;
mod v9_checkout_hooks_do_not_run_on_beads_push;
mod v9_every_checkout_shape_is_one_workspace;
mod v9_harmless_global_git_settings;
mod v9_transport_and_url_settings;

/// An environment: variables and their values.
type Env = BTreeMap<OsString, OsString>;

/// `_v9_scratch_git_config`: a scratch global git config holding `text` for this sandbox (never
/// the user's), with system config off: the variables that select it.
fn scratch_git_config(sandbox: &Sandbox, text: &str) -> Env {
    let path = sandbox.root().join("global.gitconfig");
    std::fs::write(&path, text).unwrap();
    Env::from([
        ("GIT_CONFIG_GLOBAL".into(), path.into_os_string()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ])
}

/// `_v9_git_env`: the test process's environment without `GIT_*` variables, plus the scratch git
/// config.
fn git_env(genv: &Env) -> Env {
    let mut env: Env = std::env::vars_os()
        .filter(|(key, _)| !key.as_encoded_bytes().starts_with(b"GIT_"))
        .collect();
    env.extend(genv.clone());
    env
}

/// `_v9_sh`: git ARGS with a fixed identity, in `cwd` (the test's own directory when `None`) with
/// exactly `env`; what it printed, stripped, or the test fails with git's standard error.
fn sh<S: AsRef<OsStr>>(args: &[S], cwd: Option<&Path>, env: &Env) -> String {
    let mut command = Command::new("git");
    command
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let output = command.output().expect("git starts");
    let shown: Vec<_> = args.iter().map(|a| a.as_ref().to_string_lossy()).collect();
    assert!(
        output.status.success(),
        "git {}: {}",
        shown.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git's output is UTF-8")
        .trim()
        .to_owned()
}

/// `_v9_has_ref`: whether the repository at `git_dir` has `reference`.
fn has_ref(git_dir: &Path, reference: &str) -> bool {
    Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(["rev-parse", "-q", "--verify", reference])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git starts")
        .success()
}
