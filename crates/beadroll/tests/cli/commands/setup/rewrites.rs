//! A remote's URL as git rewrites it (`url.<base>.insteadOf`, `url.<base>.pushInsteadOf`) and its own
//! push URL: bead hands git the URL as configured, so that git rewrites it as it would for `git
//! fetch NAME` and `git push NAME`, a remote's own push URL left as it is, an alias never taken for
//! a remote of the same name, and the advice it gives naming the remote as it was chosen.

use std::path::Path;

use beadroll_test_support::{Json, Machine, code, git, parse_json};

use super::{
    alias_both_ways, commit, json, mirror, set_origin, text, titles, tracked_file, workspace,
};

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

/// A remote with a push URL of its own that a `pushInsteadOf` rule would rewrite: `git push NAME`
/// leaves such a URL as it is, and so does bead, which publishes where it fetches from rather than
/// where the rule sends other URLs; so do the commands after, which go by the remote it saved.
#[test]
fn a_remotes_own_push_url_is_pushed_to_as_git_pushes_to_it() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    let tracker = sandbox.bare("tracker.git");
    let other = sandbox.bare("other.git");
    let own = format!("file://{}", text(&tracker));
    commit(&a, &["remote", "add", "tracker", text(&tracker)]);
    commit(&a, &["remote", "set-url", "--push", "tracker", &own]);
    let rewrite = format!("url.file://{}.pushInsteadOf", text(&other));
    commit(&a, &["config", &rewrite, &own]);
    let synced = a.run(["sync", "--remote", "tracker", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    let refs = |remote: &Path| {
        git(
            ["--git-dir", text(remote), "for-each-ref", "refs/beads"],
            None,
        )
    };
    assert_ne!(
        refs(&tracker),
        "",
        "nothing reached the remote's own push URL"
    );
    assert_eq!(
        refs(&other),
        "",
        "bead pushed where the rule sends other URLs"
    );
    json(&a, &["create", "after"]);
    let resynced = a.run(["sync", "--json"]);
    assert_eq!(resynced.returncode(), 0, "{}", resynced.stderr);
    assert_eq!(
        refs(&other),
        "",
        "the remote it saved was pushed to as any URL"
    );
}

/// The remote bead defines for a push to a remote's own push URL is named for that push: one of a
/// fixed name that the configuration it inherits defines already (here through `GIT_CONFIG_COUNT`)
/// adds nothing to where it pushes.
#[test]
fn the_remote_defined_for_a_push_inherits_no_urls() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    let tracker = sandbox.bare("tracker.git");
    let other = sandbox.bare("other.git");
    let own = format!("file://{}", text(&tracker));
    commit(&a, &["remote", "add", "tracker", text(&tracker)]);
    commit(&a, &["remote", "set-url", "--push", "tracker", &own]);
    let synced = a
        .cmd(["sync", "--remote", "tracker", "--json"])
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "remote.bead-coordination.pushurl")
        .env("GIT_CONFIG_VALUE_0", text(&other))
        .run();
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    let refs = |remote: &Path| {
        git(
            ["--git-dir", text(remote), "for-each-ref", "refs/beads"],
            None,
        )
    };
    assert_ne!(
        refs(&tracker),
        "",
        "nothing reached the remote's own push URL"
    );
    assert_eq!(refs(&other), "", "the push went to an inherited URL too");
}

/// Origin by default, with no push URL of its own and a `pushInsteadOf` rule for its URL: bead
/// publishes where `git push origin` goes, the rule applied, even though another remote has that
/// URL as its own push URL, which only a push to that remote leaves as it is.
#[test]
fn origin_is_pushed_to_as_git_pushes_to_it_whatever_other_remotes_say() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let pushed = sandbox.bare("push.git");
    let origin = text(sandbox.origin());
    let rewrite = format!("url.{}.pushInsteadOf", text(&pushed));
    commit(&a, &["config", &rewrite, origin]);
    let other = sandbox.bare("other.git");
    commit(&a, &["remote", "add", "other", text(&other)]);
    commit(&a, &["remote", "set-url", "--push", "other", origin]);
    json(&a, &["init", "--prefix", "ex"]);
    let refs = |remote: &Path| {
        git(
            ["--git-dir", text(remote), "for-each-ref", "refs/beads"],
            None,
        )
    };
    assert_ne!(
        refs(&pushed),
        "",
        "nothing reached where git push origin goes"
    );
    assert_eq!(
        refs(sandbox.origin()),
        "",
        "bead pushed to origin's URL as it is"
    );
}

/// The same alias seen by `init`: the URL it is rewritten to publishes a tracker, the remote of the
/// same name does not; `init` asks the first, as every exchange with the remote reaches it, and
/// joins its tracker.
#[test]
fn init_asks_a_url_that_names_another_remote_as_the_url_it_is() {
    let sandbox = crate::sandbox();
    let (first, second) = (sandbox.bare("A.git"), sandbox.bare("B.git"));
    let a = Machine::new(&sandbox, "a");
    alias_both_ways(&a, &first, &second);
    json(&a, &["init", "--prefix", "ex", "--remote", "tracker"]);
    json(&a, &["create", "shared"]);
    let b = Machine::new(&sandbox, "b");
    alias_both_ways(&b, &first, &second);
    let joined = json(&b, &["init", "--remote", "tracker"]);
    assert_eq!(joined["joined"], Json::Bool(true), "{joined:?}");
    assert_eq!(titles(&b), ["shared"]);
}

/// The advice to bring a moved repository's tracker along gives the remote as it was chosen: by its
/// name, where the URL it leads to is an alias spelled like another remote's name, which
/// `--remote` would take for that remote.
#[test]
fn the_advice_gives_a_remote_chosen_by_name_by_its_name() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    let old = workspace(&a);
    // the same name at another place, with nothing there
    let elsewhere = sandbox.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).expect("a directory");
    let moved = elsewhere.join("origin.git");
    git(["init", "-q", "--bare", "-b", "main", text(&moved)], None);
    set_origin(&a, &moved);
    let (first, second) = (sandbox.bare("A.git"), sandbox.bare("B.git"));
    alias_both_ways(&a, &first, &second);
    let synced = a.run(["sync", "--remote", "tracker", "--json"]);
    assert_eq!(code(&synced), "NOT_INITIALIZED", "{}", synced.stderr);
    let command = format!("BEAD_WORKSPACE={old} bead sync --remote tracker");
    assert!(synced.stderr.contains(&command), "{}", synced.stderr);
}

/// `init --tracked --remote` given origin's own push URL, which a `pushInsteadOf` rule would
/// rewrite: stored as `origin`, and reached as origin is, its own push URL as configured, from the
/// first publish on, as every command after reaches it.
#[test]
fn init_given_origins_own_push_url_reaches_it_as_origin() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let own = format!("file://{}", text(sandbox.origin()));
    commit(&a, &["remote", "set-url", "--push", "origin", &own]);
    let pushed = sandbox.bare("push.git");
    let rewrite = format!("url.{}.pushInsteadOf", text(&pushed));
    commit(&a, &["config", &rewrite, &own]);
    json(
        &a,
        &["init", "--tracked", "--prefix", "ex", "--remote", &own],
    );
    assert_eq!(tracked_file(&a)["remote"].as_str(), Some("origin"));
    let refs = |remote: &Path| {
        git(
            ["--git-dir", text(remote), "for-each-ref", "refs/beads"],
            None,
        )
    };
    assert_ne!(
        refs(sandbox.origin()),
        "",
        "nothing reached origin's own push URL"
    );
    assert_eq!(refs(&pushed), "", "the first publish took it for any URL");
}

/// `machine`'s origin moved to a place of the same name (`origin.git` beside the sandbox's), with
/// nothing there: its tracker stayed behind.
fn moved_without_its_tracker(sandbox: &beadroll_test_support::Sandbox, machine: &Machine) {
    let elsewhere = sandbox.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).expect("a directory");
    let moved = elsewhere.join("origin.git");
    git(["init", "-q", "--bare", "-b", "main", text(&moved)], None);
    set_origin(machine, &moved);
}

/// The advice to bring a moved repository's tracker along gives a remote chosen by name that has a
/// push URL of its own by that name: as a URL alone it would be pushed to with the `pushInsteadOf`
/// rule that sends it elsewhere.
#[test]
fn the_advice_gives_a_remote_with_its_own_push_url_by_its_name() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    let old = workspace(&a);
    moved_without_its_tracker(&sandbox, &a);
    let tracker = sandbox.bare("tracker.git");
    let own = format!("file://{}", text(&tracker));
    commit(&a, &["remote", "add", "tracker", text(&tracker)]);
    commit(&a, &["remote", "set-url", "--push", "tracker", &own]);
    let other = sandbox.bare("other.git");
    let rewrite = format!("url.file://{}.pushInsteadOf", text(&other));
    commit(&a, &["config", &rewrite, &own]);
    let synced = a.run(["sync", "--remote", "tracker", "--json"]);
    assert_eq!(code(&synced), "NOT_INITIALIZED", "{}", synced.stderr);
    let command = format!("BEAD_WORKSPACE={old} bead sync --remote tracker");
    assert!(synced.stderr.contains(&command), "{}", synced.stderr);
}

/// A remote whose URL is an alias git rewrites, and a remote of the alias's name that only the
/// configuration bead inherits defines (here through `GIT_CONFIG_COUNT`), which `sync.git` sees
/// too: bead gives git the URL as a remote's URL, never as an argument git would take for that
/// remote, and reaches what the alias is rewritten to.
#[test]
fn an_inherited_remote_never_takes_an_alias_for_its_name() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    let (first, second) = (sandbox.bare("A.git"), sandbox.bare("B.git"));
    let rewrite = format!("url.{}.insteadOf", text(&first));
    commit(&a, &["config", &rewrite, "alias"]);
    commit(&a, &["remote", "add", "tracker", "alias"]);
    let synced = a
        .cmd(["sync", "--remote", "tracker", "--json"])
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "remote.alias.url")
        .env("GIT_CONFIG_VALUE_0", text(&second))
        .run();
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    let refs = |remote: &Path| {
        git(
            ["--git-dir", text(remote), "for-each-ref", "refs/beads"],
            None,
        )
    };
    assert_ne!(
        refs(&first),
        "",
        "nothing reached what the alias is rewritten to"
    );
    assert_eq!(
        refs(&second),
        "",
        "the inherited remote took the alias for its name"
    );
}

/// A remote whose URL is a path through a symbolic link, which an `insteadOf` rule rewrites: git
/// rewrites the path as written, which bead hands it, not the path the link resolves to.
#[test]
fn a_rewritten_path_through_a_link_is_left_to_git() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    let (local, shared) = (sandbox.bare("local.git"), sandbox.bare("shared.git"));
    let link = sandbox.root().join("link.git");
    std::os::unix::fs::symlink(&local, &link).expect("a link");
    let rule = format!("url.{}.insteadOf", text(&shared));
    commit(&a, &["config", &rule, text(&link)]);
    commit(&a, &["remote", "add", "tracker", text(&link)]);
    let synced = a.run(["sync", "--remote", "tracker", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    let refs = |remote: &Path| {
        git(
            ["--git-dir", text(remote), "for-each-ref", "refs/beads"],
            None,
        )
    };
    assert_ne!(
        refs(&shared),
        "",
        "nothing reached where the rule sends the path"
    );
    assert_eq!(
        refs(&local),
        "",
        "bead resolved the link before git could rewrite it"
    );
}

/// `init --tracked --remote` given a relative path through a symbolic link whose full spelling an
/// `insteadOf` rule rewrites: stored as written, from the top of the checkout, so that the
/// commands after reach it as `init` did.
#[test]
fn init_tracked_stores_a_rewritten_path_as_written() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let (local, shared) = (sandbox.bare("local.git"), sandbox.bare("shared.git"));
    let link = a.checkout().join("link.git");
    std::os::unix::fs::symlink(&local, &link).expect("a link");
    let rule = format!("url.{}.insteadOf", text(&shared));
    commit(&a, &["config", &rule, text(&link)]);
    json(
        &a,
        &[
            "init",
            "--tracked",
            "--prefix",
            "ex",
            "--remote",
            "link.git",
        ],
    );
    assert_eq!(tracked_file(&a)["remote"].as_str(), Some("link.git"));
    let synced = a.run(["sync", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    let refs = |remote: &Path| {
        git(
            ["--git-dir", text(remote), "for-each-ref", "refs/beads"],
            None,
        )
    };
    assert_ne!(
        refs(&shared),
        "",
        "nothing reached where the rule sends the path"
    );
    assert_eq!(
        refs(&local),
        "",
        "the link was resolved before git could rewrite it"
    );
}

/// A repository moved to a place this checkout's origin reaches through a symbolic link that an
/// `insteadOf` rule rewrites: the tracker published there under the old id is adopted, origin
/// compared as a sync reaches it.
#[test]
fn a_moved_tracker_is_adopted_through_a_rewritten_origin() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "before the move"]);
    let moved = sandbox.root().join("moved.git");
    mirror(&sandbox, &moved);
    let link = sandbox.root().join("moved-link.git");
    std::os::unix::fs::symlink(&moved, &link).expect("a link");
    let b = Machine::new(&sandbox, "b");
    set_origin(&b, &link);
    let rule = format!("url.file://{}.insteadOf", text(&moved));
    commit(&b, &["config", &rule, text(&link)]);
    let synced = b.run(["sync", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    assert_eq!(titles(&b), ["before the move"]);
    assert_eq!(workspace(&b), workspace(&a));
}

/// `init --tracked --remote ./tracker.git` where an `insteadOf` rule rewrites the path's full
/// spelling, `./` included: stored so that it reads back as that very spelling, which the rule
/// still matches.
#[test]
fn init_tracked_stores_a_rewritten_path_as_it_reads_back() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let shared = sandbox.bare("shared.git");
    let spelled = format!("{}/./tracker.git", text(a.checkout()));
    let rule = format!("url.{}.insteadOf", text(&shared));
    commit(&a, &["config", &rule, &spelled]);
    json(
        &a,
        &[
            "init",
            "--tracked",
            "--prefix",
            "ex",
            "--remote",
            "./tracker.git",
        ],
    );
    assert_eq!(tracked_file(&a)["remote"].as_str(), Some("./tracker.git"));
    let synced = a.run(["sync", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    let refs = ["--git-dir", text(&shared), "for-each-ref", "refs/beads"];
    assert_ne!(
        git(refs, None),
        "",
        "nothing reached where the rule sends the path"
    );
}
