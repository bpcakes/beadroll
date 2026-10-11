//! `CompatAuditIdentity`, from the first independent audit: who the caller is when an agent was
//! started from inside another agent's session, so that both a Claude and a Codex session are in
//! the environment. The class's tests of `detect_actor` as a function of the environment are the
//! store's (`crates/beadroll-store/tests/store/commands.rs`).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use beadroll_test_support::{Run, Sandbox, json, parse_json};

/// The test's environment less every `BEAD_*`, `CLAUDE*`, `CODEX*` and `GIT_*` variable.
fn scrubbed() -> BTreeMap<OsString, OsString> {
    std::env::vars_os()
        .filter(|(key, _)| {
            let key = key.as_encoded_bytes();
            !["BEAD_", "CLAUDE", "CODEX", "GIT_"]
                .iter()
                .any(|prefix| key.starts_with(prefix.as_bytes()))
        })
        .collect()
}

/// The executable `name` on the test's own PATH, as `shutil.which` finds it.
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
/// store in `root/home`, `path` as PATH, and both a Claude and a Codex session in the environment.
fn nested_info(sandbox: &Sandbox, root: &Path, path: &OsStr) -> Run {
    let output = Command::new(sandbox.bead())
        .args(["info", "--json"])
        .current_dir(root)
        .env_clear()
        .envs(scrubbed())
        .env("BEAD_HOME", root.join("home"))
        .env("BEAD_WORKSPACE", "audit-identity")
        .env("PATH", path)
        .env("CLAUDE_CODE_SESSION_ID", "claude")
        .env("CODEX_THREAD_ID", "codex")
        .output()
        .unwrap();
    Run {
        status: output.status,
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// The process-tree walk settles nested harnesses: the nearest harness among bead's ancestors is
/// the caller, and its session the thread. The Python test replaced `subprocess.run` in-process;
/// here a fake `ps` first on PATH, which bead runs from there, answers every lookup with the
/// harness as a child of init.
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
    ] {
        let ps = fake.join("ps");
        std::fs::write(&ps, format!("#!/bin/sh\nprintf '%s\\n' '1 {proc}'\n")).unwrap();
        std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).unwrap();
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
fn missing_ps_does_not_crash_nested_actor_detection() {
    let sandbox = crate::sandbox();
    let root = sandbox.root().join("identity-nops");
    let bin_dir = root.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // git and nothing else: no `ps` to walk the process tree with.
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
