//! `PublishOnWrite`: a tracker imported on one machine sat unpublished until a manual sync, and
//! nothing said so. Now a local write (create, update, comment, dep, import, init) publishes at
//! once, as a close already did; `--offline` keeps it local; and every command reports what the
//! remote still lacks.

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, Sandbox, git, json, parse_json};

/// Machine a has initialised tracker `ex`; machine b has synced it.
fn set_up(sandbox: &Sandbox) -> (Machine, Machine) {
    let a = Machine::new(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let b = Machine::new(sandbox, "b");
    b.bead(["sync"]);
    (a, b)
}

/// The id of the issue a `--json` command answered with.
fn id(made: Option<Json>) -> String {
    made.unwrap()["id"].as_str().unwrap().to_owned()
}

/// The `field` of every issue `bead list` printed, in its order.
fn listed(machine: &Machine, field: &str) -> Vec<String> {
    let list = machine.bead(["list"]).unwrap();
    let issues = list["issues"].as_array().unwrap();
    issues
        .iter()
        .map(|i| i[field].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn writes_reach_the_other_machine_without_a_sync() {
    let sandbox = crate::sandbox();
    let (a, b) = set_up(&sandbox);
    let x = id(a.bead(["create", "x"]));
    a.bead(["comments", "add", &x, "note"]);
    let y = id(a.bead(["create", "y"]));
    a.bead(["dep", "add", &y, &x]);
    a.bead(["update", &x, "--title", "x renamed"]);
    b.bead(["sync"]);
    assert_eq!(b.show(&x)["title"].as_str(), Some("x renamed"));
    let shown = b.show(&x);
    let comments: Vec<&str> = shown["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["text"].as_str().unwrap())
        .collect();
    assert_eq!(comments, ["note"]);
    let blocked = b.bead(["blocked"]).unwrap();
    assert_eq!(blocked[0]["id"].as_str(), Some(y.as_str()));
    assert!(a.bead(["info"]).unwrap().get("unpublished").is_none());
}

#[test]
fn offline_keeps_it_local_and_says_so_until_the_next_publish() {
    let sandbox = crate::sandbox();
    let (a, b) = set_up(&sandbox);
    let made = a.run(["create", "local", "--offline", "--json"]);
    assert_eq!(made.returncode(), 0, "{}", made.stderr);
    let made = parse_json(&made.stdout).unwrap();
    assert_eq!(made["unpublished"].as_i64(), Some(1));
    let human = a.run(["create", "also local", "--offline"]);
    assert_eq!(
        human.stdout.trim().lines().last(),
        Some("2 events not yet published: bead sync")
    );
    assert_eq!(a.bead(["info"]).unwrap()["unpublished"].as_i64(), Some(2));
    // reads carry it in JSON objects only: a list of issues has no place for it
    assert_eq!(a.bead(["list"]).unwrap()["unpublished"].as_i64(), Some(2));
    let ready = a.bead(["ready"]).unwrap();
    let ready = ready.as_array().expect("ready prints a list");
    assert_eq!(ready.len(), 2);
    assert!(ready.iter().all(|issue| issue.get("unpublished").is_none()));
    b.bead(["sync"]);
    assert_eq!(b.bead(["list"]).unwrap()["issues"], Json::Array(Vec::new()));
    // an online write carries the backlog
    a.bead(["create", "published, and the two before it with it"]);
    assert!(a.bead(["info"]).unwrap().get("unpublished").is_none());
    b.bead(["sync"]);
    let list = b.bead(["list"]).unwrap();
    assert_eq!(list["issues"].as_array().unwrap().len(), 3);
}

#[test]
fn an_unreachable_remote_never_fails_a_write() {
    let sandbox = crate::sandbox();
    let (a, b) = set_up(&sandbox);
    let aside = sandbox.origin().with_file_name("origin.away");
    std::fs::rename(sandbox.origin(), &aside).unwrap();
    let made = a.run(["create", "while away", "--json"]);
    assert_eq!(made.returncode(), 0, "{}", made.stderr);
    assert_eq!(
        parse_json(&made.stdout).unwrap()["unpublished"].as_i64(),
        Some(1)
    );
    let text = a.run(["create", "still away"]).stdout;
    assert!(text.contains("not yet published"), "{text}");
    std::fs::rename(&aside, sandbox.origin()).unwrap();
    let synced = a.bead(["sync"]).unwrap();
    assert_eq!(synced["pushed"].as_i64(), Some(2));
    b.bead(["sync"]);
    let mut titles = listed(&b, "title");
    titles.sort_unstable();
    assert_eq!(titles, ["still away", "while away"]);
}

/// AS07: a close, a delete and a closing update that end a confirmed claim publish it themselves.
/// When that publish failed, the command line tried the remote once more and said "not yet
/// published" twice. Here the publish fails once (`publish.result`, the first hit): the answer
/// says so once and counts the event, and the remote has not moved, so nothing tried again.
#[test]
fn a_failed_publish_of_an_ended_claim_is_said_once_and_not_tried_again() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let origin = sandbox.origin().to_str().unwrap();
    let tip = || {
        git(
            ["--git-dir", origin, "rev-parse", "refs/beads/journal"],
            None,
        )
    };
    let endings: [&[&str]; 3] = [
        &["close", "-r", "done"],
        &["delete"],
        &["update", "--status", "closed"],
    ];
    for ending in endings {
        let key = id(a.bead(["create", "held"]));
        a.bead(["claim", &key]);
        let before = tip();
        let (verb, rest) = ending.split_first().unwrap();
        let run = a
            .cmd([*verb, key.as_str()].iter().chain(rest))
            .env(
                "BEAD_FAILPOINTS",
                "publish.result=error(REMOTE_UNREACHABLE)",
            )
            .run();
        assert_eq!(run.returncode(), 0, "{ending:?}: {}", run.stderr);
        let said = "recorded locally, not yet published (REMOTE_UNREACHABLE)";
        assert_eq!(
            run.stdout.matches(said).count(),
            1,
            "{ending:?}: {}",
            run.stdout
        );
        assert_eq!(
            run.stdout.trim().lines().last(),
            Some("1 event not yet published: bead sync"),
            "{ending:?}"
        );
        assert_eq!(tip(), before, "{ending:?}: the remote was tried again");
        a.bead(["sync"]);
        assert_ne!(tip(), before, "{ending:?}");
    }
}

#[test]
fn init_with_an_import_publishes_the_tracker_at_once() {
    let fresh = crate::sandbox();
    let source = fresh.root().join("issues.jsonl");
    let record = json!({"id": "im-1", "title": "imported", "status": "open", "priority": 2,
                        "issue_type": "task", "created_at": "2026-01-01T00:00:00Z",
                        "updated_at": "2026-01-01T00:00:00Z"});
    std::fs::write(&source, write(&record, Format::ASCII).unwrap() + "\n").unwrap();
    let one = Machine::new(&fresh, "one");
    let made = one
        .bead([
            "init",
            "--prefix",
            "im",
            "--import",
            source.to_str().unwrap(),
        ])
        .unwrap();
    assert_eq!(made["imported"].as_i64(), Some(1));
    assert!(one.bead(["info"]).unwrap().get("unpublished").is_none());
    let two = Machine::new(&fresh, "two");
    two.bead(["sync"]);
    assert_eq!(listed(&two, "id"), ["im-1"]);
    let offline = Machine::new(&fresh, "three");
    let init = offline.run(["init", "--prefix", "im", "--offline", "--json"]);
    assert_eq!(init.returncode(), 0, "{}", init.stderr);
}
