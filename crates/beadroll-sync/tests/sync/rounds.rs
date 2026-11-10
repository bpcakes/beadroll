//! Sync recovery keeps progress, spends one rebuild, and stops without retrying other errors.

mod reports;

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};

use beadroll_core::json::Object;
use beadroll_store::{Endpoint, PublicationState, Workspace};
use beadroll_sync::{Error, ROUNDS, synchronise};
use beadroll_test_support::{Machine, git};

use crate::support::{
    Gate, actor, alongside, claim_command, quoted, sandbox, settings, text, workspace,
};

/// AS08: unpacking objects must stay in the fixture even when object routing is inherited.
#[test]
fn fixture_git_writes_ignore_inherited_repository_and_object_routing() {
    beadroll_test_support::assert_git_isolation(&[
        "rounds::a_rebuild_after_merging_keeps_the_pulled_count",
    ]);
}

/// AS17: a child fixture must retain the selected binary even when its build has no default bead.
#[test]
fn fixture_git_isolation_keeps_the_selected_binary_without_a_default() {
    let sandbox = sandbox();
    let machine = Machine::new(&sandbox, "selector");
    let profile = sandbox.root().join("test-profile");
    let deps = profile.join("deps");
    std::fs::create_dir_all(&deps).unwrap();
    let relocated = deps.join("sync-test");
    std::fs::copy(std::env::current_exe().unwrap(), &relocated).unwrap();
    assert!(!profile.join("bead").exists());
    let output = Command::new(&relocated)
        .args([
            "--exact",
            "rounds::fixture_git_writes_ignore_inherited_repository_and_object_routing",
            "--nocapture",
        ])
        .current_dir(sandbox.root())
        .env_clear()
        .envs(machine.env(None, "codex"))
        .env("BEAD_UNDER_TEST", sandbox.bead())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed;"),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Unpack before emptying the commit, so git cannot fall back to a healthy packed copy. The
/// remote's object remains intact, and the live reader may finish any read it already started.
fn damage_tip(sync_git: &Path, tip: &str) {
    let packs = sync_git.join("objects/pack");
    let aside = sync_git.join("packs-before-damage");
    std::fs::create_dir(&aside).unwrap();
    let mut moved = Vec::new();
    for entry in std::fs::read_dir(packs).unwrap() {
        let path = entry.unwrap().path();
        let target = aside.join(path.file_name().unwrap());
        std::fs::rename(&path, &target).unwrap();
        if target.extension().is_some_and(|ext| ext == "pack") {
            moved.push(target);
        }
    }
    for pack in moved {
        let unpacked = beadroll_test_support::git_command()
            .arg("--git-dir")
            .arg(sync_git)
            .args(["unpack-objects", "-q"])
            .stdin(Stdio::from(std::fs::File::open(pack).unwrap()))
            .output()
            .unwrap();
        assert!(
            unpacked.status.success(),
            "{}",
            String::from_utf8_lossy(&unpacked.stderr)
        );
    }
    let object = sync_git.join("objects").join(&tip[..2]).join(&tip[2..]);
    std::fs::set_permissions(&object, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(object, b"").unwrap();
}

/// Damage after the merge must not erase its incoming count when the rebuilt round brings no
/// further events in. Only the final round's outgoing count is reported.
#[test]
fn a_rebuild_after_merging_keeps_the_pulled_count() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    let b = Machine::new(&sandbox, "b");
    a.bead(["init", "--prefix", "ex"]);
    b.bead(["sync"]);
    b.bead(["create", "from b"]);
    a.bead(["create", "from a", "--offline"]);
    let tip = git(
        [
            "--git-dir",
            sandbox.origin().to_str().unwrap(),
            "rev-parse",
            "refs/beads/journal",
        ],
        None,
    );
    let gate = Gate::new(&sandbox, "after-merge");
    let faults = format!("publish.before={};backoff@always=skip", gate.action());
    let ws = workspace(&a, sandbox.origin(), &[("BEAD_FAILPOINTS", &faults)]);
    ws.set_wrote(PublicationState::Unpublished);
    let (result, met) = alongside(
        || synchronise(&ws, &Endpoint::at(sandbox.origin())),
        |done| gate.meet(done, || damage_tip(&ws.dir.join("sync.git"), tip.trim())),
    );
    assert!(met.is_some(), "publication reached the gate");
    let synced = result.expect("the rebuilt round publishes");
    assert_eq!((synced.pulled, synced.pushed, synced.attempts), (1, 1, 2));
    assert_eq!(synced.issues, 2);
    assert!(synced.sync_repo_rebuilt.is_some());
    assert_eq!(ws.settings.failpoints.hits("backoff"), 0);
    assert!(!ws.wrote());
    b.bead(["sync"]);
    assert_eq!(a.export(), b.export());
}

/// The first fault is before the merge, the next after it. A command gets one rebuild across
/// both boundaries, and a later command can still repair the cache and publish the local edit.
#[test]
fn a_second_damaged_round_stops_without_another_rebuild() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "local edit", "--offline"]);
    let tip = git(
        [
            "--git-dir",
            sandbox.origin().to_str().unwrap(),
            "rev-parse",
            "refs/beads/journal",
        ],
        None,
    );
    let fetched = Gate::new(&sandbox, "fetched");
    let publishing = Gate::new(&sandbox, "publishing");
    let faults = format!(
        "fetch.fetched={};publish.before={};backoff@always=skip",
        fetched.action(),
        publishing.action()
    );
    let ws = workspace(&a, sandbox.origin(), &[("BEAD_FAILPOINTS", &faults)]);
    ws.set_wrote(PublicationState::Unpublished);
    let (result, met) = alongside(
        || synchronise(&ws, &Endpoint::at(sandbox.origin())),
        |done| {
            let first = fetched.meet(done, || damage_tip(&ws.dir.join("sync.git"), tip.trim()));
            let second = publishing.meet(done, || damage_tip(&ws.dir.join("sync.git"), tip.trim()));
            (first, second)
        },
    );
    assert!(
        met.0.is_some() && met.1.is_some(),
        "both damage gates reached"
    );
    assert!(
        matches!(result, Err(Error::SyncRepoDamaged(_))),
        "{result:?}"
    );
    let copies = std::fs::read_dir(&ws.dir)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("sync.git.damaged-")
        })
        .count();
    assert_eq!(copies, 1);
    assert_eq!(ws.settings.failpoints.hits("backoff"), 0);
    assert!(ws.wrote());
    a.bead(["sync"]);
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    assert_eq!(a.export(), b.export());
}

/// A rebuild consumes the first round. A workspace mismatch discovered afterwards cannot adopt
/// until a new command, whose first round still has the usual adoption opportunity.
#[test]
fn adoption_is_only_considered_on_the_first_round() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "published issue"]);
    let tip = git(
        [
            "--git-dir",
            sandbox.origin().to_str().unwrap(),
            "rev-parse",
            "refs/beads/journal",
        ],
        None,
    );
    let gate = Gate::new(&sandbox, "before-mismatch");
    let faults = format!("fetch.fetched={};backoff@always=skip", gate.action());
    let mut ws = Workspace::new(
        settings(&a, &[("BEAD_FAILPOINTS", &faults)]),
        "url-derived-id",
        None,
        Some(sandbox.origin().as_os_str().to_owned()),
        Object::new(),
    )
    .unwrap();
    ws.derived = Some(ws.id.clone());
    let (result, met) = alongside(
        || synchronise(&ws, &Endpoint::at(sandbox.origin())),
        |done| gate.meet(done, || damage_tip(&ws.dir.join("sync.git"), tip.trim())),
    );
    assert!(met.is_some(), "the first fetch reached the gate");
    assert!(
        matches!(result, Err(Error::WorkspaceMismatch { .. })),
        "{result:?}"
    );
    assert!(!ws.settings.home().unwrap().join("moved.json").exists());
    assert_eq!(ws.settings.failpoints.hits("backoff"), 0);
    let synced = synchronise(&ws, &Endpoint::at(sandbox.origin())).expect("a new command adopts");
    assert_eq!((synced.attempts, synced.issues), (1, 1));
    assert!(ws.settings.home().unwrap().join("moved.json").exists());
}

/// A git that moves the remote's journal on before every push it is asked for, so that each
/// round loses, and runs every command as it is.
fn git_that_always_loses(sandbox: &beadroll_test_support::Sandbox) -> String {
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("git"))
        .find(|git| git.is_file())
        .expect("git on PATH");
    let script = r#"#!/bin/sh
if [ "$1" = push ]; then
  ahead() {
    GIT_AUTHOR_NAME=other GIT_AUTHOR_EMAIL=other@example.invalid \
    GIT_COMMITTER_NAME=other GIT_COMMITTER_EMAIL=other@example.invalid \
    @git@ --git-dir @origin@ "$@"
  }
  tip=$(ahead rev-parse refs/beads/journal) || exit 1
  tree=$(ahead rev-parse "$tip^{tree}") || exit 1
  moved=$(ahead commit-tree "$tree" -p "$tip" -m 'someone else first') || exit 1
  ahead update-ref refs/beads/journal "$moved" || exit 1
fi
exec @git@ "$@"
"#
    .replace("@git@", &quoted(&real))
    .replace("@origin@", &quoted(sandbox.origin()));
    let folder = sandbox.root().join("git-that-always-loses");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("git"), script).unwrap();
    let mode = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(folder.join("git"), mode).unwrap();
    format!(
        "{}:{}",
        folder.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// AS03: a sync and an ownership command that lose every round give up after the last one at
/// once: they wait between two rounds, never after the last before they report the conflict.
#[test]
fn the_last_round_is_not_followed_by_a_wait() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "contended"]).unwrap()["id"]);
    a.bead(["create", "something to push", "--offline"]);
    let path = git_that_always_loses(&sandbox);
    let vars = [
        ("PATH", path.as_str()),
        ("BEAD_FAILPOINTS", "backoff@always=skip"),
    ];
    let between = u64::from(ROUNDS) - 1;

    let ws = workspace(&a, sandbox.origin(), &vars);
    ws.set_wrote(PublicationState::Unpublished);
    let lost = synchronise(&ws, &Endpoint::at(sandbox.origin())).unwrap_err();
    assert!(matches!(lost, Error::SyncConflict(_)), "{lost}");
    assert_eq!(ws.settings.failpoints.hits("backoff"), between);

    let ws = workspace(&a, sandbox.origin(), &vars);
    let lost = claim_command(&ws, &actor(&a), &key).unwrap_err();
    assert!(matches!(lost, Error::SyncConflict(_)), "{lost}");
    assert_eq!(ws.settings.failpoints.hits("backoff"), between);
}
