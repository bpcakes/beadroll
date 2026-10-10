//! The processes bead starts get standard input, output and error and nothing else, whatever bead
//! was started with: Python starts them through `subprocess` with `close_fds=True`.

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::Command;

use beadroll_test_support::Machine;

/// The git on the test's PATH.
fn real_git() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|git| git.is_file())
        .expect("git on PATH")
}

/// `text` quoted for the shell.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// bead started with descriptor 7 open, as a caller's pipe or a make jobserver would be, writes
/// through git; a git first on PATH notes, each time bead starts it, whether descriptor 7 reached
/// it, then runs the real one.
#[test]
fn git_inherits_no_descriptor_beyond_the_standard_three() {
    let sandbox = crate::sandbox();
    let m = Machine::new(&sandbox, "fds");
    m.bead(["init", "--prefix", "ex"]);
    let shims = sandbox.root().join("shims");
    std::fs::create_dir(&shims).unwrap();
    let log = sandbox.root().join("descriptor-7.log");
    let script = format!(
        // the probe runs in a subshell: dash leaves the whole script when a redirection of `:` fails
        "#!/bin/sh\nif ( : >&7 ) 2>/dev/null; then echo inherited >> {log}; else echo closed >> \
         {log}; fi\nexec {git} \"$@\"\n",
        log = quoted(&log.display().to_string()),
        git = quoted(&real_git().display().to_string()),
    );
    std::fs::write(shims.join("git"), script).unwrap();
    std::fs::set_permissions(shims.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut env = m.env(None, "codex");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::iter::once(shims).chain(std::env::split_paths(&path));
    env.insert("PATH".into(), std::env::join_paths(dirs).unwrap());
    let bead = quoted(&m.executable().display().to_string());
    let out = Command::new("/bin/sh")
        .args([
            "-c",
            &format!("exec 7</dev/null; exec {bead} create probe --json"),
        ])
        .current_dir(m.checkout())
        .env_clear()
        .envs(env)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = std::fs::read_to_string(&log).unwrap_or_default();
    let seen: Vec<&str> = seen.lines().collect();
    assert!(!seen.is_empty(), "bead started no git through the shim");
    assert!(seen.iter().all(|s| *s == "closed"), "{seen:?}");
}
