//! G11: the text a server hook prints, which git relays as `remote:` lines, never decides how a
//! push came out; git's own status line for the ref does (`docs/FINDINGS.md`). The class's other
//! test, a claiming edit rebuilt over a later observed edit, runs bead in-process with a pause
//! between publish rounds, and is ported with that failpoint.

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;

use beadroll_test_support::Sandbox;

/// `path` quoted for the shell.
fn quoted(path: &Path) -> String {
    let text = path.to_str().expect("a UTF-8 path");
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `_g11_install_post_receive`: post-receive runs after the ref update and after receive-pack sent
/// its status report. On its first run, the one that makes the directory `gate`, it prints a line
/// that looks like a refusal (the client relays it as `remote: ...`) and kills receive-pack, so the
/// client's push exits 1 although the remote applied the update.
fn install_post_receive(sandbox: &Sandbox, gate: &Path) {
    let hook = sandbox.origin().join("hooks").join("post-receive");
    let lines = [
        "#!/bin/sh".to_owned(),
        "cat >/dev/null".to_owned(),
        format!("if mkdir {} 2>/dev/null; then", quoted(gate)),
        "  echo 'mirror: ! [rejected]  refs/beads/journal -> refs/beads/journal (fetch first)' >&2"
            .to_owned(),
        "  sleep 0.2".to_owned(),
        "  kill -9 $PPID".to_owned(),
        "fi".to_owned(),
        "exit 0".to_owned(),
    ];
    std::fs::write(&hook, lines.join("\n") + "\n").unwrap();
    std::fs::set_permissions(&hook, Permissions::from_mode(0o755)).unwrap();
}

/// `_g11_remote_head`: the commit the remote's `refs/beads/journal` names, as `git rev-parse`
/// prints it; nothing when it names none.
fn remote_head(sandbox: &Sandbox) -> String {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(sandbox.origin())
        .args(["rev-parse", "refs/beads/journal"])
        .output()
        .expect("git starts");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

mod g11_server_hook_text_does_not_decide_the_push_outcome {
    use beadroll_test_support::{Machine, Sandbox};

    use super::{install_post_receive, remote_head};

    /// The class's `setUp`: machine `g11` has initialised tracker `ex`, created the issue `one` and
    /// synced. Returns the machine and the issue's key.
    fn set_up(sandbox: &Sandbox) -> (Machine, String) {
        let a = Machine::new(sandbox, "g11");
        a.bead(["init", "--prefix", "ex"]);
        let created = a.bead(["create", "one"]).expect("the new issue");
        let key = created["id"].as_str().expect("the issue's id").to_owned();
        a.bead(["sync"]);
        (a, key)
    }

    /// G11: Link.publish classifies a failed push by searching all of git's stderr, including the
    /// `remote:` lines a server hook prints. A push the remote applied, whose connection then drops
    /// after a post-receive hook printed '[rejected] ... (fetch first)', is taken as refused: the
    /// attempt and its receipt are dropped, the command is rebuilt, and the accepted release
    /// answers NOT_OWNER.
    #[test]
    fn applied_release_is_not_reported_not_owner_because_a_hook_printed_rejected() {
        let sandbox = crate::sandbox();
        let (a, key) = set_up(&sandbox);
        a.bead(["claim", &key]);
        let gate = sandbox.root().join("g11-hook-fired");
        install_post_receive(&sandbox, &gate);
        let before = remote_head(&sandbox);
        let released = a.run(["release", &key, "--json"]);
        assert!(gate.exists(), "the post-receive hook did not run");
        assert_ne!(
            remote_head(&sandbox),
            before,
            "the release did not reach the remote"
        );
        assert!(
            a.show(&key).get("claim").is_none(),
            "the release was not applied"
        );
        assert_eq!(
            released.returncode(),
            0,
            "the remote applied the release, but bead reported: {}",
            released.stderr.trim()
        );
    }
}
