//! What a remote given by name means (`--remote`, `$BEAD_REMOTE`, `.beadroll.json`): the URL it is
//! configured with, where `git push NAME` goes, read as discovery reads origin (G22); and origin's
//! own URL stored as `origin`, so that the tracked file follows origin when the repository moves.

use beadroll_test_support::{Json, Machine, git};

use super::{
    commit, commit_tracked_file, json, mirror, set_origin, text, titles, tracked_file, workspace,
};

/// `init --tracked --remote` given origin's own URL stores the name `origin`, so the tracked file
/// follows origin when the repository moves, where a stored URL would keep the old place.
#[test]
fn init_tracked_given_origins_url_stores_its_name() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let origin = text(sandbox.origin());
    json(
        &a,
        &["init", "--tracked", "--prefix", "ex", "--remote", origin],
    );
    assert_eq!(tracked_file(&a)["remote"].as_str(), Some("origin"));
    json(&a, &["create", "before the move"]);
    let moved = sandbox.root().join("moved.git");
    mirror(&sandbox, &moved);
    set_origin(&a, &moved);
    json(&a, &["create", "after the move"]);
    let synced = json(&a, &["sync"]);
    let resolved = moved.canonicalize().expect("the moved remote");
    assert_eq!(synced["remote"].as_str(), Some(text(&resolved)));
    let b = Machine::new(&sandbox, "b");
    set_origin(&b, &moved);
    commit(&a, &["push", "-q", "origin", "HEAD:main"]);
    json(&b, &["sync"]);
    assert_eq!(titles(&b), ["after the move", "before the move"]);
}

/// Origin is where `git push origin` goes (G22): a checkout whose origin fetches from one URL and
/// pushes to another coordinates through the push URL, and a tracked `origin` (what `init --tracked
/// --remote` stores for that URL) resolves to the same push URL, never the fetch one.
#[test]
fn a_tracked_origin_resolves_to_where_git_pushes() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let pushed = sandbox.bare("push.git");
    let push_url = ["remote", "set-url", "--push", "origin", text(&pushed)];
    git(push_url, Some(a.checkout()));
    let init = [
        "init",
        "--tracked",
        "--prefix",
        "ex",
        "--remote",
        text(&pushed),
    ];
    json(&a, &init);
    assert_eq!(tracked_file(&a)["remote"].as_str(), Some("origin"));
    json(&a, &["create", "coordinated"]);
    let synced = json(&a, &["sync"]);
    let resolved = pushed.canonicalize().expect("the push remote");
    assert_eq!(synced["remote"].as_str(), Some(text(&resolved)));
    let fetched = [
        "--git-dir",
        text(sandbox.origin()),
        "for-each-ref",
        "refs/beads",
    ];
    assert_eq!(
        git(fetched, None),
        "",
        "bead published to origin's fetch URL"
    );
}

/// A clone whose committed `.beadroll.json` names `"remote": "origin"`: `init --remote origin` joins
/// the tracker without writing anything, and records the remote as where `origin` leads, as a sync
/// would, so that the commands after it, which compare the two, keep working.
#[test]
fn init_joining_records_a_named_remote_as_where_it_leads() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(
        &a,
        &["init", "--tracked", "--prefix", "ex", "--remote", "origin"],
    );
    assert_eq!(tracked_file(&a)["remote"].as_str(), Some("origin"));
    commit_tracked_file(&a);
    let key = json(&a, &["create", "shared"])["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let b = Machine::new(&sandbox, "b");
    let joined = json(&b, &["init", "--remote", "origin"]);
    assert_eq!(joined["joined"], Json::Bool(true), "{joined:?}");
    let synced = b.run(["sync", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    let claimed = b.run(["claim", &key, "--json"]);
    assert_eq!(claimed.returncode(), 0, "{}", claimed.stderr);
}

/// A remote's name means its URL as configured, as discovery reads origin's: with a
/// `pushInsteadOf` that spells origin's push URL another way, `sync --remote origin` still names
/// this checkout's origin, and adopts the tracker a moved repository published under its old id.
#[test]
fn a_named_origin_is_the_configured_url_whatever_git_rewrites() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "before the move"]);
    let moved = sandbox.root().join("moved.git");
    mirror(&sandbox, &moved);
    let b = Machine::new(&sandbox, "b");
    set_origin(&b, &moved);
    let spelled = format!("url.file://{}.pushInsteadOf", text(&moved));
    commit(&b, &["config", &spelled, text(&moved)]);
    let synced = b.run(["sync", "--remote", "origin", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    assert_eq!(titles(&b), ["before the move"]);
    assert_eq!(workspace(&b), workspace(&a));
}
