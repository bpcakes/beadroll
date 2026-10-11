//! What a remote given by name means (`--remote`, `$BEAD_REMOTE`, `.beadroll.json`): the URL it is
//! configured with, where `git push NAME` goes, read as discovery reads origin (G22); origin's own
//! URL stored as `origin`, so that the tracked file follows origin when the repository moves; a
//! remote remembered as it was chosen, by its name or as a URL, an older bead's saved URL, one
//! typed or given as a path; and where git reaches a remote from, the top of the checkout. Git's
//! own rewrites of a remote's URL are in `rewrites`.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, code, git, parse_json};

use super::{
    alias_both_ways, commit, commit_tracked_file, json, mirror, set_origin, text, titles,
    tracked_file,
};

/// `store`'s `meta.json` saying that it last went through `remote`, as an older bead left it.
pub(super) fn save_remote(store: &Path, remote: &str) {
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
/// the tracker without writing anything, and keeps the remote as it was chosen, origin, which is
/// never remembered, as a sync would, so that the commands after it, which compare the tracked
/// remote with the one remembered, keep working.
#[test]
fn init_joining_keeps_a_named_remote_as_it_was_chosen() {
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

/// A store that goes through a remote by its name, whose `.beadroll.json` is changed to that
/// remote's own push URL: the same URL, but another choice, pushed to as any URL, which a
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

/// `init --remote NAME` where NAME is a configured remote's name and also, as a path from where
/// `init` runs, origin's repository: the remote of that name, which wins over a same-named path.
#[test]
fn init_given_a_remotes_name_that_is_also_origins_path_reaches_the_remote() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let inside = a.checkout().join("tracker");
    git(["init", "-q", "--bare", "-b", "main", text(&inside)], None);
    set_origin(&a, &inside);
    let named = sandbox.bare("named.git");
    commit(&a, &["remote", "add", "tracker", text(&named)]);
    json(&a, &["init", "--prefix", "ex", "--remote", "tracker"]);
    let refs = |remote: &Path| {
        git(
            ["--git-dir", text(remote), "for-each-ref", "refs/beads"],
            None,
        )
    };
    assert_ne!(refs(&named), "", "nothing reached the remote named");
    assert_eq!(refs(&inside), "", "init took the name for origin's path");
}

/// A remote whose URL is an alias git rewrites and also another remote's name: bead reaches the
/// rewritten URL, as git does, and so does every command after, which goes by the remote's name it
/// remembered and gives git that remote's URL as a URL, never as the remote it also names.
#[test]
fn a_url_that_names_another_remote_stays_a_url() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    let (first, second) = (sandbox.bare("A.git"), sandbox.bare("B.git"));
    alias_both_ways(&a, &first, &second);
    let synced = a.run(["sync", "--remote", "tracker", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    json(&a, &["create", "after"]);
    let refs = |remote: &Path| {
        git(
            ["--git-dir", text(remote), "for-each-ref", "refs/beads"],
            None,
        )
    };
    assert_ne!(
        refs(&first),
        "",
        "nothing reached the URL the alias is rewritten to"
    );
    assert_eq!(
        refs(&second),
        "",
        "a later command took the remote's URL for a remote's name"
    );
}

/// A store that goes through a remote whose URL is an alias git rewrites, whose `.beadroll.json` is
/// changed to name the remote of the alias's name: that URL's spelling, another remote; refused.
#[test]
fn a_tracked_name_spelled_like_the_saved_alias_is_a_change() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let (first, second) = (sandbox.bare("A.git"), sandbox.bare("B.git"));
    alias_both_ways(&a, &first, &second);
    json(
        &a,
        &["init", "--tracked", "--prefix", "ex", "--remote", "tracker"],
    );
    let mut config = tracked_file(&a);
    if let Json::Object(fields) = &mut config {
        fields.insert("remote".into(), Json::Str("alias".into()));
    }
    let body = beadroll_core::json::write(&config, beadroll_core::json::Format::INDENTED);
    std::fs::write(
        a.checkout().join(".beadroll.json"),
        body.expect("JSON text"),
    )
    .unwrap();
    assert_eq!(code(&a.run(["sync", "--json"])), "CONFIG");
    let refs = ["--git-dir", text(&second), "for-each-ref", "refs/beads"];
    assert_eq!(
        git(refs, None),
        "",
        "the store moved to the remote of the alias's name"
    );
}

/// A store that coordinates through a repository, whose `.beadroll.json` is changed to name a
/// remote that git fetches from there but pushes elsewhere (`insteadOf` and `pushInsteadOf` for
/// its URL): another place for every write; refused (`CONFIG`) until changed deliberately.
#[test]
fn a_tracked_name_that_pushes_elsewhere_is_a_change() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    let (first, second) = (sandbox.bare("A.git"), sandbox.bare("B.git"));
    json(&a, &["sync", "--remote", text(&first)]);
    let fetched = format!("url.{}.insteadOf", text(&first));
    let pushed = format!("url.{}.pushInsteadOf", text(&second));
    commit(&a, &["config", &fetched, "alias"]);
    commit(&a, &["config", &pushed, "alias"]);
    commit(&a, &["remote", "add", "tracker", "alias"]);
    let tracked = a.checkout().join(".beadroll.json");
    std::fs::write(tracked, "{\"remote\": \"tracker\"}\n").expect("a tracked file");
    assert_eq!(code(&a.run(["sync", "--json"])), "CONFIG");
    let refs = ["--git-dir", text(&second), "for-each-ref", "refs/beads"];
    assert_eq!(git(refs, None), "", "a write went where the name pushes");
}

/// `init --remote` given origin's own push URL, which a `pushInsteadOf` rule would rewrite, when
/// its first publish fails: nothing is published after it through the URL as typed, which the
/// rule would send elsewhere; the events wait, counted, for the next sync.
#[test]
fn init_publishes_only_where_it_chose_to() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let own = format!("file://{}", text(sandbox.origin()));
    commit(&a, &["remote", "set-url", "--push", "origin", &own]);
    let pushed = sandbox.bare("push.git");
    let rewrite = format!("url.{}.pushInsteadOf", text(&pushed));
    commit(&a, &["config", &rewrite, &own]);
    // from now on origin refuses every push
    let hook = sandbox.origin().join("hooks").join("pre-receive");
    std::fs::write(&hook, "#!/bin/sh\necho refused >&2\nexit 1\n").expect("a hook");
    let executable = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(&hook, executable).expect("an executable hook");
    let run = a.run(["init", "--prefix", "ex", "--remote", &own, "--json"]);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    let result = parse_json(&run.stdout).expect("a result");
    let behind = result["unpublished"].as_i64();
    assert!(behind.is_some_and(|n| n > 0), "{result:?}");
    let refs = ["--git-dir", text(&pushed), "for-each-ref", "refs/beads"];
    assert_eq!(
        git(refs, None),
        "",
        "a publish after init's went where the rule sends the URL"
    );
}

/// A store an older bead left going through a URL that an `insteadOf` rule rewrites, whose
/// `.beadroll.json` names a remote that another rule rewrites to that very URL: git rewrites once,
/// so the saved URL reaches one place and the name another; refused (`CONFIG`).
#[test]
fn a_tracked_name_reaching_another_place_than_the_saved_url_is_a_change() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let init = json(&a, &["init", "--prefix", "ex", "--offline"]);
    let store = PathBuf::from(init["path"].as_str().expect("a path"));
    let (named, saved) = (sandbox.bare("B.git"), sandbox.bare("C.git"));
    let to_named = format!("url.{}.insteadOf", text(&named));
    let to_saved = format!("url.{}.insteadOf", text(&saved));
    commit(&a, &["config", &to_named, "alias:repo"]);
    commit(&a, &["config", &to_saved, text(&named)]);
    commit(&a, &["remote", "add", "tracker", "alias:repo"]);
    save_remote(&store, text(&named));
    let tracked = a.checkout().join(".beadroll.json");
    std::fs::write(tracked, "{\"remote\": \"tracker\"}\n").expect("a tracked file");
    assert_eq!(code(&a.run(["sync", "--json"])), "CONFIG");
}

/// A remote whose own push URL is origin's URL, which a `pushInsteadOf` rule sends elsewhere when
/// it is origin's: a sync through that remote is remembered by its name, not forgotten as origin,
/// so the commands after publish where it did.
#[test]
fn a_remote_pushed_to_by_origins_url_is_not_forgotten_as_origin() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    let origin = text(sandbox.origin());
    let pushed = sandbox.bare("push.git");
    let rewrite = format!("url.{}.pushInsteadOf", text(&pushed));
    commit(&a, &["config", &rewrite, origin]);
    let tracker = sandbox.bare("tracker.git");
    commit(&a, &["remote", "add", "tracker", text(&tracker)]);
    commit(&a, &["remote", "set-url", "--push", "tracker", origin]);
    let synced = a.run(["sync", "--remote", "tracker", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    json(&a, &["create", "after"]);
    let refs = ["--git-dir", text(&pushed), "for-each-ref", "refs/beads"];
    assert_eq!(
        git(refs, None),
        "",
        "a later command reached origin's URL as origin"
    );
}

/// A remote whose URL is a symbolic link to origin's repository, which an `insteadOf` rule
/// rewrites to another repository: a sync through it is remembered by its name, not forgotten as
/// origin, which the link alone would make it, and the writes after go where the sync went.
#[test]
fn a_remote_rewritten_away_from_origin_is_not_forgotten_as_origin() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    let tip = || {
        git(
            [
                "--git-dir",
                text(sandbox.origin()),
                "rev-parse",
                "refs/beads/journal",
            ],
            None,
        )
    };
    let before = tip();
    let coord = sandbox.bare("coord.git");
    let link = sandbox.root().join("link.git");
    std::os::unix::fs::symlink(sandbox.origin(), &link).expect("a link");
    let rule = format!("url.{}.insteadOf", text(&coord));
    commit(&a, &["config", &rule, text(&link)]);
    commit(&a, &["remote", "add", "tracker", text(&link)]);
    json(&a, &["sync", "--remote", "tracker"]);
    json(&a, &["create", "after"]);
    assert_eq!(tip(), before, "a write after the sync went to origin");
}
