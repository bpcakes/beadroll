//! What a remote given by name means (`--remote`, `$BEAD_REMOTE`, `.beadroll.json`): the URL it is
//! configured with, where `git push NAME` goes, read as discovery reads origin (G22); origin's own
//! URL stored as `origin`, so that the tracked file follows origin when the repository moves; a
//! remote saved, typed or given as a path; and where git reaches a remote from, the top of the
//! checkout. Git's own rewrites of a remote's URL are in `rewrites`.

use std::path::{Path, PathBuf};

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, code, git, parse_json};

use super::{commit, commit_tracked_file, json, mirror, set_origin, text, titles, tracked_file};

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

/// A checkout whose directory's name ends in a space: git reaches the remote from that directory,
/// as `git rev-parse --show-toplevel` prints it, the space kept.
#[test]
fn a_checkout_whose_name_ends_in_a_space_reaches_its_remote() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let spaced = sandbox.clone("spaced ", None);
    let init = a
        .cmd(["init", "--prefix", "ex", "--json"])
        .cwd(&spaced)
        .run();
    assert_eq!(init.returncode(), 0, "{}", init.stderr);
    let synced = a.cmd(["sync", "--json"]).cwd(&spaced).run();
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    let refs = [
        "--git-dir",
        text(sandbox.origin()),
        "for-each-ref",
        "refs/beads",
    ];
    assert_ne!(git(refs, None), "", "nothing reached the remote");
}

/// A remote whose URL is a relative path that is also another remote's name (`remote.tracker.url
/// = origin`, a repository `origin` in the checkout): bead reads the URL as the path it is, not as
/// the remote `origin`.
#[test]
fn a_remote_url_that_names_another_remote_is_still_a_path() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let inside = a.checkout().join("origin");
    git(["init", "-q", "--bare", "-b", "main", text(&inside)], None);
    commit(&a, &["remote", "add", "tracker", "origin"]);
    let init = [
        "init",
        "--tracked",
        "--prefix",
        "ex",
        "--remote",
        "tracker",
        "--json",
    ];
    let run = a.run(init);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    let refs = |remote: &Path| {
        git(
            ["--git-dir", text(remote), "for-each-ref", "refs/beads"],
            None,
        )
    };
    assert_ne!(
        refs(&inside),
        "",
        "nothing reached the path the remote names"
    );
    assert_eq!(
        refs(sandbox.origin()),
        "",
        "bead went to the remote origin instead"
    );
}

/// A relative path typed in a subdirectory names a place from there, also where nothing is: bead
/// reaches it from there, not from the top of the checkout, where git starts.
#[test]
fn a_typed_relative_path_is_read_from_where_it_was_typed() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    // what `../tracker.git` names from the top of the checkout, the sandbox's `a-main`
    let beside = sandbox.bare("tracker.git");
    let below = a.checkout().join("src");
    std::fs::create_dir(&below).expect("a subdirectory");
    let sync = ["sync", "--remote", "../tracker.git", "--json"];
    let synced = a.cmd(sync).cwd(&below).run();
    assert_ne!(synced.returncode(), 0, "{}", synced.stdout);
    let refs = ["--git-dir", text(&beside), "for-each-ref", "refs/beads"];
    assert_eq!(
        git(refs, None),
        "",
        "bead reached the path from the top of the checkout"
    );
}

/// A store an older bead left with a remote's name saved as `init --offline --remote NAME` gave it,
/// no sync having replaced it with its URL yet: the name, which no rule rewrites, is that remote.
#[test]
fn a_remote_name_an_older_bead_saved_is_that_remote() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let tracker = sandbox.bare("tracker.git");
    commit(&a, &["remote", "add", "tracker", text(&tracker)]);
    let init = json(&a, &["init", "--prefix", "ex", "--offline"]);
    let store = PathBuf::from(init["path"].as_str().expect("a path"));
    save_remote(&store, "tracker");
    let synced = a.run(["sync", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    let refs = ["--git-dir", text(&tracker), "for-each-ref", "refs/beads"];
    assert_ne!(
        git(refs, None),
        "",
        "the saved name was not taken for its remote"
    );
}

/// A store that last went through a remote's own push URL, whose `.beadroll.json` is changed from
/// that remote's name to the URL itself: the same URL, but pushed to as any URL, which a
/// `pushInsteadOf` rule can send elsewhere; refused (`CONFIG`) until changed deliberately.
#[test]
fn a_tracked_remote_pushed_to_otherwise_is_a_change() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let tracker = sandbox.bare("tracker.git");
    let own = format!("file://{}", text(&tracker));
    commit(&a, &["remote", "add", "tracker", text(&tracker)]);
    commit(&a, &["remote", "set-url", "--push", "tracker", &own]);
    json(
        &a,
        &["init", "--tracked", "--prefix", "ex", "--remote", "tracker"],
    );
    let mut config = tracked_file(&a);
    if let Json::Object(fields) = &mut config {
        fields.insert("remote".into(), Json::Str(own));
    }
    let body = write(&config, Format::INDENTED).expect("JSON text");
    std::fs::write(a.checkout().join(".beadroll.json"), body).expect("the tracked file");
    assert_eq!(code(&a.run(["sync", "--json"])), "CONFIG");
}
