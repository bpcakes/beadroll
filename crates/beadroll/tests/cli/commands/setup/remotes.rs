//! What a remote given by name means (`--remote`, `$BEAD_REMOTE`, `.beadroll.json`): the URL it is
//! configured with, where `git push NAME` goes, read as discovery reads origin (G22); and origin's
//! own URL stored as `origin`, so that the tracked file follows origin when the repository moves.

use std::path::{Path, PathBuf};

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, code, git, parse_json};

use super::{
    commit, commit_tracked_file, json, mirror, set_origin, text, titles, tracked_file, workspace,
};

/// `store`'s `meta.json` saying that it last went through `remote`, as an older bead left it.
fn save_remote(store: &Path, remote: &str) {
    let path = store.join("meta.json");
    let body = std::fs::read_to_string(&path).expect("a meta.json");
    let mut meta = parse_json(&body).expect("valid JSON");
    if let Json::Object(fields) = &mut meta {
        fields.insert("remote".into(), Json::Str(remote.to_owned()));
    }
    let body = write(&meta, Format::INDENTED).expect("JSON text");
    std::fs::write(&path, body).expect("the meta.json");
}

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

/// A tracked remote's name whose URL git rewrites (`url.<base>.insteadOf`), in a store an older
/// bead left: it saved the name as `git remote get-url` gives it, rewritten, which is the same
/// remote as the URL configured, so bead goes on through it. A push URL of the name's own is
/// another place, which older beads never went through: moving there stays deliberate.
#[test]
fn a_tracked_remote_saved_as_git_rewrote_it_is_still_the_same_remote() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let tracker = sandbox.bare("tracker.git");
    let rewrite = format!("url.{}/.insteadOf", text(sandbox.root()));
    commit(&a, &["config", &rewrite, "company:"]);
    commit(&a, &["remote", "add", "tracker", "company:tracker.git"]);
    let init = ["init", "--tracked", "--prefix", "ex", "--remote", "tracker"];
    let store = PathBuf::from(json(&a, &init)["path"].as_str().expect("a path"));
    save_remote(&store, text(&tracker));
    let synced = a.run(["sync", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    let elsewhere = sandbox.bare("elsewhere.git");
    let push_url = ["remote", "set-url", "--push", "tracker", text(&elsewhere)];
    commit(&a, &push_url);
    save_remote(&store, text(&tracker));
    assert_eq!(code(&a.run(["sync", "--json"])), "CONFIG");
}

/// A remote whose URL reads as a relative path that git rewrites (`url.<base>.insteadOf`): bead
/// leaves it to git, rather than make a path of it from the top of the checkout, which git could
/// no longer rewrite.
#[test]
fn a_remote_git_rewrites_is_not_read_as_a_path() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let tracker = sandbox.bare("tracker.git");
    let rewrite = format!("url.{}/.insteadOf", text(sandbox.root()));
    commit(&a, &["config", &rewrite, "company/"]);
    commit(&a, &["remote", "add", "tracker", "company/tracker.git"]);
    let init = ["init", "--tracked", "--prefix", "ex", "--remote", "tracker"];
    let run = a.run(init.iter().chain(&["--json"]));
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    json(&a, &["create", "published"]);
    let published = ["--git-dir", text(&tracker), "for-each-ref", "refs/beads"];
    assert_ne!(
        git(published, None),
        "",
        "nothing reached the rewritten remote"
    );
    let result = parse_json(&run.stdout).expect("a result");
    assert_eq!(result.get("unpublished"), None, "{result:?}");
}

/// A remote whose URL git rewrites to a relative path (`url."../".insteadOf`), with bead run in a
/// subdirectory: the path is read from the top of the checkout, as bead reads every relative remote
/// path, not from where bead runs.
#[test]
fn a_rewrite_to_a_relative_path_is_read_from_the_top() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let tracker = sandbox.bare("tracker.git");
    commit(&a, &["config", "url.../.insteadOf", "company:"]);
    commit(&a, &["remote", "add", "tracker", "company:tracker.git"]);
    let below = a.checkout().join("src");
    std::fs::create_dir(&below).expect("a subdirectory");
    // the checkout is the sandbox's `a-main`, beside tracker.git
    let init = [
        "init",
        "--tracked",
        "--prefix",
        "ex",
        "--remote",
        "tracker",
        "--json",
    ];
    let run = a.cmd(init).cwd(&below).run();
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    let refs = ["--git-dir", text(&tracker), "for-each-ref", "refs/beads"];
    assert_ne!(
        git(refs, None),
        "",
        "the tracker did not reach the rewritten remote"
    );
    let synced = a.cmd(["sync", "--json"]).cwd(&below).run();
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
}
