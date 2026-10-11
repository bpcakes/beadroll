//! A remote kept as it was chosen: by its name, which every command looks up again, as it looks up
//! origin; as a URL; or as origin's own URL, which is origin. Origin is never remembered, also in
//! another spelling of its repository an older bead saved (G25).

use std::path::{Path, PathBuf};

use beadroll_test_support::{Json, Machine, code, git, parse_json};

use super::remotes::save_remote;
use super::{commit, json, text, tracked_file};

/// The `meta.json` of the store at `path`, as `init` printed it.
fn meta(path: &str) -> Json {
    let body = std::fs::read_to_string(Path::new(path).join("meta.json")).expect("a meta.json");
    parse_json(&body).expect("valid JSON")
}

/// Whether the repository at `remote` holds a tracker.
fn holds_tracker(remote: &Path) -> bool {
    let refs = ["--git-dir", text(remote), "for-each-ref", "refs/beads"];
    !git(refs, None).is_empty()
}

/// A remote chosen by its name is remembered by it, and the commands after look its URL up again,
/// as they look origin's up: `git remote set-url` takes the tracker's remote with it.
#[test]
fn a_remote_chosen_by_its_name_follows_its_url() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let init = json(&a, &["init", "--prefix", "ex"]);
    let store = init["path"].as_str().expect("a path").to_owned();
    let (first, second) = (sandbox.bare("first.git"), sandbox.bare("second.git"));
    commit(&a, &["remote", "add", "tracker", text(&first)]);
    json(&a, &["sync", "--remote", "tracker"]);
    assert_eq!(meta(&store)["remote"].as_str(), Some("tracker"));
    commit(&a, &["remote", "set-url", "tracker", text(&second)]);
    json(&a, &["create", "after the move"]);
    assert!(holds_tracker(&second), "a write did not follow the name");
}

/// `sync --remote` given origin's own push URL, which a `pushInsteadOf` rule rewrites when it is
/// given alone: it is origin, pushed to as `git push origin` pushes there, without the rule, and
/// forgotten as origin is, so the writes after go there too.
#[test]
fn origins_own_url_given_is_reached_as_origin() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let init = json(&a, &["init", "--prefix", "ex"]);
    let store = init["path"].as_str().expect("a path").to_owned();
    let own = format!("file://{}", text(sandbox.origin()));
    commit(&a, &["remote", "set-url", "--push", "origin", &own]);
    let pushed = sandbox.bare("push.git");
    let rewrite = format!("url.{}.pushInsteadOf", text(&pushed));
    commit(&a, &["config", &rewrite, &own]);
    json(&a, &["create", "--offline", "waiting"]);
    json(&a, &["sync", "--remote", &own]);
    json(&a, &["create", "after"]);
    assert!(
        !holds_tracker(&pushed),
        "a publish went where the rule sends the URL"
    );
    assert_eq!(meta(&store)["remote"], Json::Null, "origin was remembered");
}

/// A remote given as origin's repository spelled otherwise (https where origin is ssh), as an
/// older bead saved it before origin changed spelling: it is origin, never remembered, unless a
/// rule rewrites that spelling, which can send it elsewhere.
#[test]
fn origin_spelled_otherwise_is_origin_unless_a_rule_rewrites_it() {
    let sandbox = crate::sandbox();
    let https = "https://example.invalid/org/repo.git";
    let remembered = |machine: &Machine| {
        let ssh = "git@example.invalid:org/repo.git";
        commit(machine, &["remote", "set-url", "origin", ssh]);
        let args = ["init", "--prefix", "ex", "--offline", "--remote", https];
        let init = json(machine, &args);
        meta(init["path"].as_str().expect("a path"))["remote"].clone()
    };
    let a = Machine::new(&sandbox, "a");
    assert_eq!(remembered(&a), Json::Null, "origin was remembered");
    let b = Machine::new(&sandbox, "b");
    let rule = "url.ssh://elsewhere.invalid/.insteadOf";
    commit(&b, &["config", rule, "https://example.invalid/"]);
    assert_eq!(remembered(&b), Json::Str(https.into()));
}

/// `init --tracked --remote` given origin's repository spelled otherwise (`file://` before its
/// path), which a rule rewrites to another repository: a URL of its own, stored as typed and
/// reached where git takes it, never taken for origin by its spelling.
#[test]
fn origins_path_spelled_as_a_url_is_a_url_of_its_own() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let spelled = format!("file://{}", text(sandbox.origin()));
    let elsewhere = sandbox.bare("elsewhere.git");
    let rule = format!("url.file://{}.insteadOf", text(&elsewhere));
    commit(&a, &["config", &rule, &spelled]);
    let init = ["init", "--tracked", "--prefix", "ex", "--remote", &spelled];
    json(&a, &init);
    assert_eq!(tracked_file(&a)["remote"].as_str(), Some(spelled.as_str()));
    assert!(
        holds_tracker(&elsewhere),
        "init did not go where git takes the URL"
    );
    assert!(
        !holds_tracker(sandbox.origin()),
        "init took the URL for origin"
    );
}

/// A store that goes through a repository, whose `.beadroll.json` is changed to another whose path
/// differs only by `.git`, which the URL-derived id's spellings would take for the same: another
/// repository, another choice; refused (`CONFIG`).
#[test]
fn a_tracked_path_one_dot_git_apart_is_a_change() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let (plain, dotted) = (sandbox.bare("tracker"), sandbox.bare("tracker.git"));
    let init = [
        "init",
        "--tracked",
        "--prefix",
        "ex",
        "--remote",
        text(&dotted),
    ];
    json(&a, &init);
    let tracked = a.checkout().join(".beadroll.json");
    let body = std::fs::read_to_string(&tracked).expect("a .beadroll.json");
    std::fs::write(&tracked, body.replace(text(&dotted), text(&plain))).expect("the file");
    assert_eq!(code(&a.run(["sync", "--json"])), "CONFIG");
    assert!(
        !holds_tracker(&plain),
        "the store moved to the other repository"
    );
}

/// A tracked remote's name that git rewrites to a relative path (`url."../".insteadOf`), in a store
/// an older bead left with the path it led to saved, run in a subdirectory: git's answers for the
/// name are read from the top of the checkout, where git reaches the remote, so the saved path is
/// still that remote.
#[test]
fn a_name_rewritten_to_a_relative_path_is_compared_from_the_top() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    // the checkout is the sandbox's `a-main`, beside tracker.git
    let tracker = sandbox.bare("tracker.git");
    commit(&a, &["config", "url.../.insteadOf", "company:"]);
    commit(&a, &["remote", "add", "tracker", "company:tracker.git"]);
    let init = ["init", "--tracked", "--prefix", "ex", "--remote", "tracker"];
    let store = PathBuf::from(json(&a, &init)["path"].as_str().expect("a path"));
    save_remote(&store, text(&tracker));
    let below = a.checkout().join("src");
    std::fs::create_dir(&below).expect("a subdirectory");
    let synced = a.cmd(["sync", "--json"]).cwd(&below).run();
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
}

/// Origin configured as a relative path that a rule rewrites (`url.<base>.insteadOf`): bead
/// reaches it as `git push origin` does, by the spelling configured, which git rewrites, whether
/// origin is named or nothing is, so a write after `init --remote origin` goes where it went.
#[test]
fn origin_is_reached_as_configured_where_a_rule_rewrites_it() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let tracker = sandbox.bare("tracker.git");
    let rule = format!("url.{}/.insteadOf", text(sandbox.root()));
    commit(&a, &["config", &rule, "company/"]);
    commit(&a, &["remote", "set-url", "origin", "company/tracker.git"]);
    json(&a, &["init", "--prefix", "ex", "--remote", "origin"]);
    let tip = || {
        let ask = [
            "--git-dir",
            text(&tracker),
            "rev-parse",
            "refs/beads/journal",
        ];
        git(ask, None)
    };
    let before = tip();
    json(&a, &["create", "after"]);
    assert_ne!(
        tip(),
        before,
        "a write went elsewhere than git takes origin"
    );
}
