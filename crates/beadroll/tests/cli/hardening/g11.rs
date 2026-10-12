//! G11: server hook output relayed as `remote:` lines must not determine the push outcome; git's
//! status for the ref does. A second test pauses between publish rounds to check that an accepted
//! claiming edit is not rebuilt over a later edit.

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
    use std::path::Path;

    use beadroll_test_support::{Json, Machine, Sandbox, parse_json};

    use super::{install_post_receive, remote_head};
    use crate::hardening::gate::{Gate, beside};

    /// The class's `setUp`: machine `g11` (made by `make`) has initialised tracker `ex`, created
    /// the issue `one` and synced. Returns the machine and the issue's key.
    fn set_up(sandbox: &Sandbox, make: fn(&Sandbox, &str) -> Machine) -> (Machine, String) {
        let a = make(sandbox, "g11");
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
        let (a, key) = set_up(&sandbox, Machine::new);
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

    /// Every JSON line in files under `dir` whose names contain `.json`.
    fn events_under(dir: &Path, found: &mut Vec<Json>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                events_under(&path, found);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy();
            if !name.contains(".json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let lines = text.lines().filter(|line| !line.trim().is_empty());
            found.extend(lines.map(|line| parse_json(line).unwrap()));
        }
    }

    /// G11: misclassifying an accepted `update --claim --title` as refused can replay it over a
    /// later edit. Pause each retry wait with `backoff@always=pause-until(...)`; at the first
    /// wait, machine b observes and changes the title. Later waits pass through the open gate.
    /// Correctly recognising acceptance causes no second round, so b never runs.
    #[test]
    fn applied_claiming_edit_is_not_rebuilt_over_a_later_observed_edit() {
        let sandbox = crate::sandbox();
        let (a, key) = set_up(&sandbox, crate::rust_only);
        let b = crate::rust_only(&sandbox, "g11b");
        b.bead(["sync"]);
        let workspace = a.store().file_name().unwrap().to_owned();
        let fired = sandbox.root().join("g11-hook-fired");
        install_post_receive(&sandbox, &fired);
        let between = Gate::new(&sandbox, "g11-between-rounds");
        let failpoints = format!("backoff@always={}", between.action());
        let argv = ["update", &key, "--claim", "--title", "accepted first"];
        let (updated, later) = beside(
            || {
                let update = a.cmd(argv).env("BEAD_WORKSPACE", &workspace);
                update.env("BEAD_FAILPOINTS", &failpoints).run()
            },
            |finished| {
                between.meet(finished, || {
                    b.bead(["sync"]);
                    assert_eq!(
                        b.show(&key)["title"].as_str(),
                        Some("accepted first"),
                        "b did not see the accepted edit"
                    );
                    b.bead(["update", &key, "--title", "later observed edit"]);
                    b.bead(["sync"]);
                })
            },
        );
        assert!(!updated.crashed(), "{}", updated.stderr);
        assert_eq!(
            updated.returncode(),
            0,
            "the remote accepted the update, but bead reported {}",
            updated.stderr.trim()
        );
        assert!(fired.exists(), "the post-receive hook did not run");
        b.bead(["sync"]);
        a.bead(["sync"]);
        let mut events = Vec::new();
        events_under(&a.store().join("events"), &mut events);
        let sets = events.iter().filter(|event| {
            event["op"].as_str() == Some("set")
                && event["data"]["fields"]["title"].as_str() == Some("accepted first")
        });
        assert_eq!(
            sets.count(),
            1,
            "the accepted edit was dropped as refused and emitted a second time"
        );
        let expected = if later.is_some() {
            "later observed edit"
        } else {
            "accepted first"
        };
        assert_eq!(
            a.show(&key)["title"].as_str(),
            Some(expected),
            "the rebuilt edit overwrote a later edit that had seen the accepted one"
        );
    }
}
