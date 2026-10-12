//! Setting up a repository's tracker, the golden path (roll-dih8.13): `bead init` once, from any
//! checkout, the workspace derived from the origin URL; on every other checkout and machine `bead
//! sync`, or an `init` that joins the tracker the remote already publishes instead of starting a
//! second one. A tracked `.beadroll.json` is the exception, and `info` and `prime` say when it does
//! not reach the other checkouts. A repository that moves keeps its tracker. What a remote's name
//! means is in `remotes`, git's rewrites of its URL in `rewrites`, the warning in `warnings`.

use std::path::Path;

use beadroll_test_support::{Json, Machine, Sandbox, code, git, parse_json};

use crate::hardening::gate::{Gate, beside};

mod chosen;
mod publication;
mod remotes;
mod rewrites;
mod warnings;

/// `path` as the text git and bead are given.
fn text(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

/// `bead ARGS --json` on `machine`: what it printed; a failure panics.
fn json(machine: &Machine, args: &[&str]) -> Json {
    machine.bead(args).expect("bead printed a document")
}

/// The titles of the issues `machine` lists, sorted.
fn titles(machine: &Machine) -> Vec<String> {
    let listed = json(machine, &["list", "--all", "--limit", "0"]);
    let issues = listed["issues"].as_array().expect("a list of issues");
    let mut titles: Vec<String> = issues
        .iter()
        .map(|issue| issue["title"].as_str().expect("a title").to_owned())
        .collect();
    titles.sort();
    titles
}

/// The workspace `machine`'s tracker is.
fn workspace(machine: &Machine) -> String {
    let info = json(machine, &["info"]);
    info["workspace"].as_str().expect("a workspace").to_owned()
}

/// How many events `machine`'s tracker holds.
fn events(machine: &Machine) -> i64 {
    json(machine, &["info"])["events"]
        .as_i64()
        .expect("a count")
}

/// The `.beadroll.json` at the top of `machine`'s checkout.
fn tracked_file(machine: &Machine) -> Json {
    let path = machine.checkout().join(".beadroll.json");
    let body = std::fs::read_to_string(path).expect("a .beadroll.json");
    parse_json(&body).expect("valid JSON")
}

/// A copy of the sandbox's remote at `path`, every ref with it, as a rename, a transfer or a
/// mirror push moves a repository.
fn mirror(sandbox: &Sandbox, path: &Path) {
    let from = text(sandbox.origin());
    git(["clone", "-q", "--mirror", from, text(path)], None);
}

/// `machine`'s checkout now fetches from and pushes to `url`, as after `git remote set-url`.
fn set_origin(machine: &Machine, url: &Path) {
    git(
        ["remote", "set-url", "origin", text(url)],
        Some(machine.checkout()),
    );
}

/// `git ARGS` in `machine`'s checkout, as a committer.
fn commit(machine: &Machine, args: &[&str]) {
    let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
    git(identity.iter().chain(args), Some(machine.checkout()));
}

/// The tracked file `a` commits, pushed to origin's default branch, so that clones read it.
fn commit_tracked_file(a: &Machine) {
    commit(a, &["add", ".beadroll.json"]);
    commit(a, &["commit", "-q", "-m", "tracker"]);
    commit(a, &["push", "-q", "origin", "HEAD:main"]);
}

/// `machine`'s checkout with a remote `tracker` whose URL is `alias`, which an `insteadOf` rule
/// rewrites to `first`, and a remote named `alias` at `second`.
fn alias_both_ways(machine: &Machine, first: &Path, second: &Path) {
    let rewrite = format!("url.{}.insteadOf", text(first));
    commit(machine, &["config", &rewrite, "alias"]);
    commit(machine, &["remote", "add", "alias", text(second)]);
    commit(machine, &["remote", "add", "tracker", "alias"]);
}

/// A second machine's `init` joins the tracker the remote already publishes: no second `init`
/// event, the first machine's prefix whatever `--prefix` asks, its issues there at once.
#[test]
fn init_on_another_machine_joins_the_published_tracker() {
    let sandbox = crate::sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "shared"]);
    let before = events(&a);
    let joined = json(&b, &["init", "--prefix", "other"]);
    assert_eq!(joined["joined"], Json::Bool(true), "{joined:?}");
    assert_eq!(joined["prefix"].as_str(), Some("ex"));
    assert_eq!(titles(&b), ["shared"]);
    json(&a, &["sync"]);
    assert_eq!(events(&a), before, "a second init event was written");
    assert_eq!(workspace(&b), workspace(&a));
}

/// jig-sh's case: a tracked id in a `.beadroll.json` that was never committed. A clone without the
/// file derives another id from the URL; its `init` adopts the published tracker instead of
/// starting a second one, and both machines write to the one tracker.
#[test]
fn init_on_a_clone_without_the_tracked_file_adopts_the_published_tracker() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--tracked", "--prefix", "ex"]);
    let id = tracked_file(&a)["workspace"].clone();
    json(&a, &["create", "from a"]);
    let b = Machine::new(&sandbox, "b");
    assert!(!b.checkout().join(".beadroll.json").exists());
    let joined = json(&b, &["init"]);
    assert_eq!(joined["joined"], Json::Bool(true), "{joined:?}");
    assert_eq!(joined["workspace"], id);
    json(&b, &["create", "from b"]);
    json(&a, &["sync"]);
    assert_eq!(titles(&a), ["from a", "from b"]);
}

/// A repository that moved with its refs (a rename, a transfer, a mirror push): a machine whose
/// origin is the new URL derives a new id, and its `init` adopts the tracker published under the
/// old one, as its first `sync` would.
#[test]
fn init_after_a_move_adopts_the_tracker_published_under_the_old_id() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "before the move"]);
    let moved = sandbox.root().join("moved.git");
    mirror(&sandbox, &moved);
    let b = Machine::new(&sandbox, "b");
    set_origin(&b, &moved);
    let joined = json(&b, &["init"]);
    assert_eq!(joined["joined"], Json::Bool(true), "{joined:?}");
    assert_eq!(joined["workspace"].as_str(), Some(workspace(&a).as_str()));
    assert_eq!(titles(&b), ["before the move"]);
    assert!(b.home().join("moved.json").exists());
}

/// `init --tracked` where the remote publishes a URL-derived tracker joins it but writes no
/// `.beadroll.json`: naming that id would stop working when the URL changed (the file would then be
/// refused as another repository's id), and minting another would split the tracker. So again
/// once the tracker is here, brought by the refused run or started here by a plain `init`.
#[test]
fn init_tracked_never_pins_a_url_derived_tracker() {
    let sandbox = crate::sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "shared"]);
    for machine in [&b, &b, &a] {
        let refused = machine.run(["init", "--tracked", "--json"]);
        assert_eq!(code(&refused), "CONFIG", "{}", refused.stderr);
        assert!(refused.stderr.contains("URL-derived"), "{}", refused.stderr);
        assert!(!machine.checkout().join(".beadroll.json").exists());
    }
    assert_eq!(titles(&b), ["shared"]);
    assert_eq!(workspace(&b), workspace(&a));
    assert_eq!(events(&b), events(&a));
}

/// `init --tracked` on a clone that lacks the tracked file keeps the tracked id the remote
/// publishes, instead of minting another and splitting the tracker.
#[test]
fn init_tracked_keeps_the_tracked_id_the_remote_publishes() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--tracked", "--prefix", "ex"]);
    let id = tracked_file(&a)["workspace"].clone();
    json(&a, &["create", "shared"]);
    let b = Machine::new(&sandbox, "b");
    let joined = json(&b, &["init", "--tracked"]);
    assert_eq!(joined["joined"], Json::Bool(true), "{joined:?}");
    assert_eq!(tracked_file(&b)["workspace"], id);
    assert_eq!(titles(&b), ["shared"]);
}

/// Once the remote has said it publishes the tracker, a failure to join it fails `init`, which
/// then starts nothing: after a move, the adoption is remembered before the adopted tracker is
/// fetched, and an import written to the store the checkout had would be where no later command
/// looks. The next sync joins the tracker, without the import.
#[test]
fn init_starts_nothing_when_joining_the_published_tracker_fails() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "before the move"]);
    let moved = sandbox.root().join("moved.git");
    mirror(&sandbox, &moved);
    let b = crate::rust_only(&sandbox, "b");
    set_origin(&b, &moved);
    let records = sandbox.root().join("import.jsonl");
    let record = concat!(
        r#"{"id":"ex-imp","title":"imported","status":"open","priority":2,"issue_type":"task","#,
        r#""created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}"#,
    );
    std::fs::write(&records, format!("{record}\n")).expect("an export");
    // the adopted tracker's fetch, the second of the run, fails
    let failed = b
        .cmd(["init", "--import", text(&records), "--json"])
        .env(
            "BEAD_FAILPOINTS",
            "fetch.result@nth(2)=error(REMOTE_UNREACHABLE)",
        )
        .run();
    assert_eq!(code(&failed), "REMOTE_UNREACHABLE", "{}", failed.stderr);
    json(&b, &["sync"]);
    assert_eq!(titles(&b), ["before the move"]);
    assert_eq!(workspace(&b), workspace(&a));
}

/// A tracked id other than the one the remote publishes cannot be adopted: `init` refuses it,
/// saying how to name the published tracker, and starts nothing.
#[test]
fn init_refuses_a_published_tracker_it_cannot_adopt() {
    let sandbox = crate::sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    json(&a, &["init", "--prefix", "ex"]);
    let other = "{\"workspace\": \"01a12a00-0000-7000-8000-000000000000\"}\n";
    std::fs::write(b.checkout().join(".beadroll.json"), other).expect("a tracked file");
    let refused = b.run(["init", "--json"]);
    assert_eq!(code(&refused), "WORKSPACE_MISMATCH", "{}", refused.stderr);
    assert!(
        refused.stderr.contains("BEAD_WORKSPACE="),
        "{}",
        refused.stderr
    );
    assert_eq!(code(&b.run(["list", "--json"])), "NOT_INITIALIZED");
}

/// A repository that moved without its tracker's ref (an import of branches and tags only): sync
/// finds none published and names this machine's tracker of the same name; the command it gives
/// publishes that tracker there, and a plain sync then adopts it.
#[test]
fn a_move_without_the_tracker_says_how_to_bring_it_along() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "before the move"]);
    let old = workspace(&a);
    // the same name at another place, with the branches only
    let elsewhere = sandbox.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).expect("a directory");
    let moved = elsewhere.join("origin.git");
    git(["init", "-q", "--bare", "-b", "main", text(&moved)], None);
    let origin = text(sandbox.origin());
    git(
        [
            "--git-dir",
            origin,
            "push",
            "-q",
            text(&moved),
            "refs/heads/*:refs/heads/*",
        ],
        None,
    );
    set_origin(&a, &moved);
    let synced = a.run(["sync", "--json"]);
    assert_eq!(code(&synced), "NOT_INITIALIZED", "{}", synced.stderr);
    // origin, as the sync chose it: by its name, which this checkout now leads to the new place
    let command = format!("BEAD_WORKSPACE={old} bead sync --remote origin");
    assert!(synced.stderr.contains(&command), "{}", synced.stderr);
    let advised = ["sync", "--remote", "origin", "--json"];
    let published = a.cmd(advised).env("BEAD_WORKSPACE", &old).run();
    assert_eq!(published.returncode(), 0, "{}", published.stderr);
    json(&a, &["sync"]);
    assert_eq!(titles(&a), ["before the move"]);
    assert_eq!(workspace(&a), old);
}

/// Where this machine has no tracker yet, bead says to fetch it with `sync` first and to start one
/// with `init` only if there is none; `prime` says the same.
#[test]
fn not_initialized_points_to_sync_first() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let listed = a.run(["list", "--json"]);
    assert_eq!(code(&listed), "NOT_INITIALIZED");
    let error = parse_json(&listed.stderr).expect("a JSON error");
    let message = error["error"]["message"].as_str().expect("a message");
    let expected = "no tracker for this repository here yet: run `bead sync` to fetch it";
    assert!(message.starts_with(expected), "{message}");
    let primed = a.run(["prime"]);
    let said = "No tracker here yet (NOT_INITIALIZED): run `bead sync` to fetch";
    assert!(primed.stdout.contains(said), "{}", primed.stdout);
}

/// The ref gone between the adoption and the adopted tracker's fetch: the adoption is already
/// remembered, so `init` starts nothing, rather than write where the next command does not look.
#[test]
fn init_starts_nothing_when_the_adopted_tracker_vanishes() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "before the move"]);
    let moved = sandbox.root().join("moved.git");
    mirror(&sandbox, &moved);
    let b = crate::rust_only(&sandbox, "b");
    set_origin(&b, &moved);
    let gate = Gate::new(&sandbox, "adopted-fetch");
    let failpoints = format!("fetch.result@nth(2)={}", gate.action());
    let gone = [
        "--git-dir",
        text(&moved),
        "update-ref",
        "-d",
        "refs/beads/journal",
    ];
    let (run, met) = beside(
        || {
            let init = b.cmd(["init", "--json"]);
            init.env("BEAD_FAILPOINTS", &failpoints).run()
        },
        |finished| gate.meet(finished, || git(gone, None)),
    );
    assert!(
        met.is_some(),
        "bead never reached the adopted tracker's fetch"
    );
    assert_eq!(code(&run), "NOT_INITIALIZED", "{}", run.stderr);
    assert_eq!(code(&b.run(["list", "--json"])), "NOT_INITIALIZED");
}

/// An `init` that adopted another workspace counts what it left unpublished there: an import the
/// remote refused to take is reported as unpublished, as for any other write.
#[test]
fn init_reports_what_it_left_unpublished_in_the_adopted_tracker() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "before the move"]);
    let moved = sandbox.root().join("moved.git");
    mirror(&sandbox, &moved);
    // from now on the moved remote refuses every push
    let hook = moved.join("hooks").join("pre-receive");
    std::fs::write(&hook, "#!/bin/sh\necho refused >&2\nexit 1\n").expect("a hook");
    let executable = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&hook, executable).expect("an executable hook");
    let b = Machine::new(&sandbox, "b");
    set_origin(&b, &moved);
    let records = sandbox.root().join("import.jsonl");
    let record = concat!(
        r#"{"id":"ex-imp","title":"imported","status":"open","priority":2,"issue_type":"task","#,
        r#""created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}"#,
    );
    std::fs::write(&records, format!("{record}\n")).expect("an export");
    let run = b.run(["init", "--import", text(&records), "--json"]);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    let result = parse_json(&run.stdout).expect("a result");
    assert_eq!(result["joined"], Json::Bool(true), "{result:?}");
    let behind = result["unpublished"].as_i64();
    assert!(behind.is_some_and(|n| n > 0), "{result:?}");
}

/// `init --tracked --remote` given a path in a subdirectory: the path goes into the tracked file
/// relative to the top of the checkout, and the tracker the remote publishes is reached where the
/// path leads, not where that stored spelling would lead from the subdirectory.
#[test]
fn init_tracked_in_a_subdirectory_joins_the_tracker_its_path_leads_to() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--tracked", "--prefix", "ex"]);
    json(&a, &["create", "shared"]);
    commit_tracked_file(&a);
    let b = Machine::new(&sandbox, "b");
    let below = b.checkout().join("src");
    std::fs::create_dir(&below).expect("a subdirectory");
    // the checkout is the sandbox's `b-main`, beside origin.git
    let init = [
        "init",
        "--tracked",
        "--remote",
        "../../origin.git",
        "--json",
    ];
    let run = b.cmd(init).cwd(&below).run();
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    let result = parse_json(&run.stdout).expect("a result");
    assert_eq!(result["joined"], Json::Bool(true), "{result:?}");
    assert_eq!(titles(&b), ["shared"]);
    assert_eq!(events(&b), events(&a));
}

/// A moved repository's tracker brought along to a remote named on the command line: the advice
/// names that remote too, where the tracker left behind would not go by itself (it goes to origin,
/// or to the remote it last went through), and following it publishes the tracker there.
#[test]
fn the_advice_to_bring_a_tracker_along_names_the_remote_asked() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "before the move"]);
    let old = workspace(&a);
    // the same name at another place, so that the tracker of that name here is named
    let elsewhere = sandbox.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).expect("a directory");
    let moved = elsewhere.join("origin.git");
    git(["init", "-q", "--bare", "-b", "main", text(&moved)], None);
    set_origin(&a, &moved);
    let tracker = sandbox.bare("tracker.git");
    let synced = a.run(["sync", "--remote", text(&tracker), "--json"]);
    assert_eq!(code(&synced), "NOT_INITIALIZED", "{}", synced.stderr);
    let command = format!("BEAD_WORKSPACE={old} bead sync --remote {}", text(&tracker));
    assert!(synced.stderr.contains(&command), "{}", synced.stderr);
    let advised = ["sync", "--remote", text(&tracker), "--json"];
    let published = a.cmd(advised).env("BEAD_WORKSPACE", &old).run();
    assert_eq!(published.returncode(), 0, "{}", published.stderr);
    let refs = ["--git-dir", text(&tracker), "for-each-ref", "refs/beads"];
    assert_ne!(
        git(refs, None),
        "",
        "the tracker did not reach the remote asked"
    );
}

/// `init --tracked` on a clone whose plain `init` already adopted the tracked tracker the remote
/// publishes (the clone lacks the file): the file names that tracker, as when `init --tracked`
/// joins it itself, never a new id beside it.
#[test]
fn init_tracked_names_the_tracker_an_earlier_init_adopted() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--tracked", "--prefix", "ex"]);
    json(&a, &["create", "shared"]);
    let b = Machine::new(&sandbox, "b");
    let joined = json(&b, &["init"]);
    assert_eq!(joined["joined"], Json::Bool(true), "{joined:?}");
    json(&b, &["init", "--tracked"]);
    assert_eq!(tracked_file(&b)["workspace"], tracked_file(&a)["workspace"]);
    assert_eq!(workspace(&b), workspace(&a));
    assert_eq!(titles(&b), ["shared"]);
    assert_eq!(events(&b), events(&a));
}

/// `init --tracked --remote` given, in a subdirectory, a relative path that leads nowhere yet:
/// read from the subdirectory as typed, never from the top of the checkout, where the same
/// spelling names another place; the tracked file stores it from the top, where it is read.
#[test]
fn init_tracked_in_a_subdirectory_reads_a_missing_path_from_there() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    // what `../tracker.git` names from the top of the checkout, the sandbox's `a-main`
    let beside = sandbox.bare("tracker.git");
    let below = a.checkout().join("src");
    std::fs::create_dir(&below).expect("a subdirectory");
    let init = [
        "init",
        "--tracked",
        "--prefix",
        "ex",
        "--remote",
        "../tracker.git",
    ];
    let run = a.cmd(init.iter().chain(&["--json"])).cwd(&below).run();
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    assert_eq!(tracked_file(&a)["remote"].as_str(), Some("tracker.git"));
    let refs = ["--git-dir", text(&beside), "for-each-ref", "refs/beads"];
    assert_eq!(git(refs, None), "", "init published beside the checkout");
}

/// A repository moved to a place whose own push URL origin now has: the tracker adopted there is
/// reached through origin, which is never remembered, its own push URL included, so that the
/// commands after follow origin as it is configured then.
#[test]
fn an_adopted_tracker_does_not_remember_origin_by_its_push_url() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "before the move"]);
    let moved = sandbox.root().join("moved.git");
    mirror(&sandbox, &moved);
    let b = Machine::new(&sandbox, "b");
    set_origin(&b, &moved);
    let own = format!("file://{}", text(&moved));
    commit(&b, &["remote", "set-url", "--push", "origin", &own]);
    json(&b, &["sync"]);
    assert_eq!(workspace(&b), workspace(&a));
    let meta = b.home().join(workspace(&b)).join("meta.json");
    let meta = parse_json(&std::fs::read_to_string(meta).expect("a meta.json")).expect("JSON");
    assert_eq!(meta.get("remote"), None, "{meta:?}");
}
