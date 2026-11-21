//! Who the caller is when an agent was started from inside another agent's session, so that more
//! than one harness's session variables are in the environment. Tests of `detect_actor` as a
//! function of the environment are the store's (`crates/beadroll-store/tests/store/commands.rs`).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use beadroll_test_support::{Run, Sandbox, executable, json, parse_json};

/// The test's environment less every `BEAD_*`, `CLAUDE*`, `CODEX*`, `CURSOR*` and `GIT_*` variable.
fn scrubbed() -> BTreeMap<OsString, OsString> {
    std::env::vars_os()
        .filter(|(key, _)| {
            let key = key.as_encoded_bytes();
            !["BEAD_", "CLAUDE", "CODEX", "CURSOR", "GIT_"]
                .iter()
                .any(|prefix| key.starts_with(prefix.as_bytes()))
        })
        .collect()
}

/// The executable path of `python3` on the test's PATH, or `None` if unavailable. An alternate
/// `BEAD_UNDER_TEST` may use an interpreter launcher.
fn python() -> Option<PathBuf> {
    let output = Command::new("python3")
        .args(["-I", "-c", "import sys; print(sys.executable)"])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let mut executable = output.stdout;
    if executable.pop() != Some(b'\n') || executable.is_empty() {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(executable)))
}

/// The executable `name` on the test's own PATH.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// `bead info --json` run in `root`, which is no repository, on workspace `audit-identity` with its
/// store in `root/home`, `path` as PATH, and `sessions` in the environment.
fn info_with_sessions(
    sandbox: &Sandbox,
    root: &Path,
    path: &OsStr,
    sessions: &[(&str, &str)],
) -> Run {
    let mut command = Command::new(sandbox.bead());
    command
        .args(["info", "--json"])
        .current_dir(root)
        .env_clear()
        .envs(scrubbed())
        .env("BEAD_HOME", root.join("home"))
        .env("BEAD_WORKSPACE", "audit-identity")
        .env("PATH", path);
    for (key, value) in sessions {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    Run {
        status: output.status,
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// `bead info --json` with Claude, Codex and Cursor sessions all in the environment.
fn nested_info(sandbox: &Sandbox, root: &Path, path: &OsStr) -> Run {
    info_with_sessions(
        sandbox,
        root,
        path,
        &[
            ("CLAUDE_CODE_SESSION_ID", "claude"),
            ("CODEX_THREAD_ID", "codex"),
            ("CURSOR_CONVERSATION_ID", "cursor"),
        ],
    )
}

/// The process-tree walk selects the nearest ancestor harness and its session. A fake `ps` first
/// on PATH answers each lookup with that harness as a child of init.
#[test]
fn nearest_harness_mocked_process_tree() {
    let sandbox = crate::sandbox();
    let root = sandbox.root().join("identity-nearest");
    let fake = root.join("bin");
    std::fs::create_dir_all(&fake).unwrap();
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::iter::once(fake.clone()).chain(std::env::split_paths(&inherited));
    let path = std::env::join_paths(dirs).unwrap();
    for (proc, want) in [
        ("/opt/bin/claude --worker", "claude"),
        ("/opt/bin/codex --worker", "codex"),
        ("node /opt/claude/cli.js", "claude"),
        ("/opt/bin/cursor-agent --worker", "cursor"),
        ("/Applications/Cursor.app/Contents/MacOS/Cursor", "cursor"),
    ] {
        let ps = fake.join("ps");
        executable(&ps, format!("#!/bin/sh\nprintf '%s\\n' '1 {proc}'\n"));
        let p = nested_info(&sandbox, &root, &path);
        assert_eq!(p.returncode(), 0, "{proc}: {}", p.stderr);
        assert_eq!(
            parse_json(&p.stdout).unwrap()["actor"],
            json!({"agent": want, "thread": want}),
            "{proc}"
        );
    }
}

#[test]
fn cursor_session_is_detected_from_environment() {
    let sandbox = crate::sandbox();
    let root = sandbox.root().join("identity-cursor");
    std::fs::create_dir_all(&root).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let p = info_with_sessions(
        &sandbox,
        &root,
        &path,
        &[
            ("CURSOR_AGENT", "1"),
            ("CURSOR_CONVERSATION_ID", "agent-1234"),
        ],
    );
    assert_eq!(p.returncode(), 0, "{}", p.stderr);
    assert_eq!(
        parse_json(&p.stdout).unwrap()["actor"],
        json!({"agent": "cursor", "thread": "agent-1234"})
    );
}

#[test]
fn missing_ps_does_not_crash_nested_actor_detection() {
    let sandbox = crate::sandbox();
    let root = sandbox.root().join("identity-nops");
    let bin_dir = root.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // git, and python3 for a launcher under test, but no `ps` to walk the process tree with
    if let Some(python) = python() {
        std::os::unix::fs::symlink(python, bin_dir.join("python3")).unwrap();
    }
    let git = which("git").expect("git on the test's PATH");
    std::os::unix::fs::symlink(git, bin_dir.join("git")).unwrap();
    let p = nested_info(&sandbox, &root, bin_dir.as_os_str());
    assert_eq!(
        p.returncode(),
        0,
        "missing ps aborts nested identity: {}",
        p.stderr
    );
    assert_eq!(
        parse_json(&p.stdout).unwrap()["actor"],
        json!({"agent": "codex", "thread": "codex"})
    );
}
