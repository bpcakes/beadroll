//! Setting up a repository's tracker, the golden path (roll-dih8.13): `bead init` once, from any
//! checkout, the workspace derived from the origin URL; on every other checkout and machine `bead
//! sync`, or an `init` that joins the tracker the remote already publishes instead of starting a
//! second one. A tracked `.beadroll.json` is the exception, and `info` and `prime` say when it does
//! not reach the other checkouts. A repository that moves keeps its tracker.

use std::path::Path;

use beadroll_test_support::{Json, Machine, Sandbox, code, git, parse_json};

use crate::hardening::gate::{Gate, beside};

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
/// refused as another repository's id), and minting another would split the tracker.
#[test]
fn init_tracked_never_pins_a_url_derived_tracker() {
    let sandbox = crate::sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    json(&a, &["init", "--prefix", "ex"]);
    json(&a, &["create", "shared"]);
    let refused = b.run(["init", "--tracked", "--json"]);
    assert_eq!(code(&refused), "CONFIG", "{}", refused.stderr);
    assert!(refused.stderr.contains("URL-derived"), "{}", refused.stderr);
    assert!(!b.checkout().join(".beadroll.json").exists());
    assert_eq!(titles(&b), ["shared"]);
    assert_eq!(workspace(&b), workspace(&a));
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
    let command = format!("BEAD_WORKSPACE={old} bead sync");
    assert!(synced.stderr.contains(&command), "{}", synced.stderr);
    let published = a.cmd(["sync", "--json"]).env("BEAD_WORKSPACE", &old).run();
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

/// `info` and `prime` warn while the checkout's `.beadroll.json` does not reach the other checkouts
/// and machines: not committed, only staged, committed but not on origin's default branch, changed
/// after its commit; and say nothing once it is there.
#[test]
fn info_and_prime_warn_about_a_tracked_file_the_others_do_not_see() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    // whatever the user's git hides from its own status
    commit(&a, &["config", "status.showUntrackedFiles", "no"]);
    json(&a, &["init", "--tracked", "--prefix", "ex"]);
    let warning = || -> Option<String> {
        json(&a, &["info"])["tracked_config_warning"]
            .as_str()
            .map(str::to_owned)
    };
    let said = warning().expect("a warning");
    assert!(
        said.starts_with(".beadroll.json is not committed"),
        "{said}"
    );
    let primed = a.run(["prime"]);
    let first = "Warning: .beadroll.json is not committed";
    assert!(primed.stdout.contains(first), "{}", primed.stdout);
    commit(&a, &["add", ".beadroll.json"]);
    let said = warning().expect("a warning");
    assert!(
        said.starts_with(".beadroll.json is added but not committed"),
        "{said}"
    );
    commit(&a, &["commit", "-q", "-m", "tracker"]);
    let said = warning().expect("a warning");
    assert!(said.contains("not on origin's default branch"), "{said}");
    commit(&a, &["push", "-q", "origin", "HEAD:main"]);
    assert_eq!(warning(), None);
    assert!(!a.run(["prime"]).stdout.contains("Warning:"));
    let mut changed = tracked_file(&a);
    if let Json::Object(fields) = &mut changed {
        fields.insert("where".into(), Json::Bool(false));
    }
    let body = beadroll_core::json::write(&changed, beadroll_core::json::Format::INDENTED)
        .expect("JSON text");
    std::fs::write(a.checkout().join(".beadroll.json"), body).expect("the tracked file");
    let said = warning().expect("a warning");
    assert!(
        said.starts_with(".beadroll.json differs from its committed copy"),
        "{said}"
    );
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
