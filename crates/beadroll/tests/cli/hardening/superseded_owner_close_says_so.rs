//! G65: an owner's close that a takeover has already superseded does not answer as if it closed.

use beadroll_test_support::{Json, Machine, git, json, parse_json};

/// G65: an owner's close (or delete) published right after another machine's takeover is
/// ignored everywhere and listed under stale, as REVIEWING section 2 decides; but the command
/// still answers '✓ Closed' / status closed with exit 0, although its own publish has just
/// fetched the takeover and this machine now shows the issue in_progress under the new owner.
#[test]
fn close_superseded_by_a_takeover_does_not_answer_closed() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let b = Machine::new(&sandbox, "b");
    a.bead(["init", "--prefix", "so"]);
    let created = a.bead(["create", "x"]).expect("the created issue");
    let key = created["id"].as_str().expect("an id").to_owned();
    a.bead(["sync"]);
    b.bead(["sync"]);
    a.bead(["claim", &key]);
    b.bead(["sync"]);
    b.bead(["takeover", &key, "--reason", "a looked dead"]);
    let closed = a.run(["close", &key, "--reason", "done", "--json"]);
    let shown = a.show(&key);
    let stale: Vec<String> = shown.get("stale").map_or_else(Vec::new, |stale| {
        let entries = stale.as_array().expect("stale is a list");
        let op =
            |entry: &beadroll_test_support::Json| entry["op"].as_str().expect("an op").to_owned();
        entries.iter().map(op).collect()
    });
    let status = shown["status"].as_str().expect("a status");
    assert_eq!((status, stale), ("in_progress", vec!["close".to_owned()]));
    // exit 0, as the decision in REVIEWING §2 has it, saying it was superseded and not closed
    assert_eq!(closed.returncode(), 0, "{}", closed.stderr);
    let result = parse_json(&closed.stdout).expect("close printed JSON");
    assert_ne!(
        result[0]["status"].as_str(),
        Some("closed"),
        "the close answered closed, yet it was superseded and ignored: {}",
        closed.stdout
    );
    let said = result[0]["superseded"].as_str().unwrap_or_default();
    assert!(
        said.contains("another owner took over"),
        "{}",
        closed.stdout
    );
}

/// An owner's close made with nowhere to publish to (a tracker with no remote) is answered with
/// what replay made of it as well, though nothing was exchanged (roll-e3pu): replay can put it
/// before the claim it names, when an event of a cycle of causes, waiting forever, keeps that
/// claim off the frontier the close builds on and the clock is behind the claim. The close is then
/// ignored, the issue still in progress, and the answer says so: the close superseded, the issue
/// in progress. What words it says it in is not this test's: no other owner took over here.
#[test]
fn a_close_replay_ignores_is_superseded_with_nothing_published() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    git(["remote", "remove", "origin"], Some(a.checkout()));
    a.bead(["init", "--prefix", "so"]);
    let key = a.bead(["create", "x"]).expect("the created issue")["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    a.bead(["claim", &key]);
    let shown = a.show(&key);
    let claim = shown["claim"]["id"].as_str().expect("a claim").to_owned();
    // the create is the last event of a's own journal: the issue its id names
    let segment = a.store().join("events").join(a.id()).join("000001.jsonl");
    let journal = std::fs::read_to_string(&segment).expect("a's journal");
    let created = journal.lines().last().expect("a record");
    let issue = parse_json(created).expect("an event")["issue"]
        .as_str()
        .expect("an issue")
        .to_owned();
    let (one, two) = (
        beadroll_test_support::events::id_at(1_790_000_000_001, 1),
        beadroll_test_support::events::id_at(1_790_000_000_002, 2),
    );
    let said = |id: &str, deps: Vec<&str>| -> Json {
        let deps: Vec<Json> = deps.into_iter().map(Json::from).collect();
        json!({
            "v": 1, "id": id, "replica": "0000000f", "deps": deps,
            "actor": {"agent": "test", "thread": "f"},
            "op": "comment", "issue": (issue.as_str()), "data": {"text": "waits"}
        })
    };
    let lines = [said(&one, vec![&claim, &two]), said(&two, vec![&one])];
    let folder = a.store().join("events").join("0000000f");
    beadroll_store::private_dir(&folder).expect("the foreign journal's folder");
    let text = beadroll_store::encode_events(&lines).expect("journal lines");
    std::fs::write(folder.join("000001.jsonl"), text).expect("the foreign journal");
    let closed = (a.cmd(["close", &key, "--json"]))
        .env("BEAD_FAILPOINTS", "clock=offset(-1d)")
        .run();
    assert_eq!(closed.returncode(), 0, "{}", closed.stderr);
    assert_eq!(a.show(&key)["status"].as_str(), Some("in_progress"));
    let result = parse_json(&closed.stdout).expect("close printed JSON");
    assert!(
        result[0]["superseded"].as_str().is_some(),
        "{}",
        closed.stdout
    );
    assert_eq!(
        result[0]["status"].as_str(),
        Some("in_progress"),
        "{}",
        closed.stdout
    );
}
