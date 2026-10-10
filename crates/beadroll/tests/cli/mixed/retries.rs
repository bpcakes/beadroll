//! Retries across a switch of binaries (`docs/RUST-PORT.md` §4.2 item 4): a machine that moves
//! from one implementation to the other on the same home and actor recognises the receipt the
//! first left, so an accepted request is never applied twice, after a sync, a later failure, a
//! takeover or a stop after confirmation; and a request whose content changed is a new one.

use std::path::Path;
use std::process::Output;

use beadroll_test_support::{Json, Machine, parse_json};

use super::receipts::{CRASH_AFTER, call, ok, receipts, tracker};
use super::{python, rust};

/// The claiming update whose acknowledgement is lost, its description on standard input.
const RETRIED: [&str; 7] = [
    "update",
    "ex-a",
    "--claim",
    "--title",
    "accepted title",
    "--description-file",
    "-",
];

/// The code of a failed `bead ... --json`.
fn code(out: &Output) -> String {
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let error = parse_json(&String::from_utf8_lossy(&out.stderr)).expect("a JSON error");
    error["error"]["code"].as_str().expect("a code").to_owned()
}

/// The ops machine a's actor wrote on ex-a, as `log` lists them.
fn ops_by_a(bead: &Path, a: &Machine) -> Vec<String> {
    let log = ok(bead, a, &["log", "ex-a", "--limit", "0"]);
    log.as_array()
        .expect("a list")
        .iter()
        .filter(|row| row["thread"].as_str() == Some("a-thread-0001"))
        .map(|row| row["op"].as_str().expect("an op").to_owned())
        .collect()
}

/// `first` leaves an accepted claiming update whose acknowledgement was lost; another machine
/// edits the issue after it; `first` syncs, which leaves no ownership pending but keeps the
/// receipt. Then `second` runs on the same home as the same actor: another actor's identical
/// command is refused, and the retry is recognised and retired without applying anything again,
/// so the newer edit stands.
fn retried_by_the_other(first: &Path, second: &Path) {
    let sandbox = crate::sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    tracker(first, &sandbox, &a);
    let crashed = call(first, &a, &RETRIED, "from stdin\n", &[CRASH_AFTER]);
    assert_eq!(
        crashed.status.code(),
        Some(99),
        "{}",
        String::from_utf8_lossy(&crashed.stderr)
    );
    ok(second, &b, &["sync"]);
    ok(second, &b, &["update", "ex-a", "--title", "newer title"]);
    ok(second, &b, &["sync"]);
    ok(first, &a, &["sync"]);
    assert_eq!(
        ok(first, &a, &["info"])["pending_ownership"],
        Json::Array(Vec::new())
    );
    assert_eq!(
        receipts(&a).len(),
        1,
        "sync keeps an accepted attempt's receipt"
    );

    let mut other = RETRIED.to_vec();
    other.push("--json");
    let refused = call(
        second,
        &a,
        &other,
        "from stdin\n",
        &[("BEAD_THREAD", "other-thread")],
    );
    assert_eq!(code(&refused), "ALREADY_CLAIMED");
    // a later command of the actor that fails leaves the receipt in place
    let failed = call(second, &a, &["claim", "ex-zzz", "--json"], "", &[]);
    assert_eq!(code(&failed), "ISSUE_NOT_FOUND");
    assert_eq!(receipts(&a).len(), 1);

    let retry = call(second, &a, &RETRIED, "from stdin\n", &[]);
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    let shown = &ok(second, &a, &["show", "ex-a"])[0];
    assert_eq!(shown["title"].as_str(), Some("newer title"));
    assert_eq!(shown["description"].as_str(), Some("from stdin\n"));
    assert_eq!(shown["claim"]["thread"].as_str(), Some("a-thread-0001"));
    let ops = ops_by_a(second, &a);
    assert_eq!(
        ops,
        ["create", "claim", "set"],
        "the request was applied again"
    );
    assert!(
        receipts(&a).is_empty(),
        "a recognised retry retires its receipt"
    );
}

/// The same command with other content on standard input is another request: `second` applies it,
/// and the receipt `first` left is dropped as stale once it has gone through, as is its own.
fn changed_content_is_a_new_request(first: &Path, second: &Path) {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    tracker(first, &sandbox, &a);
    let crashed = call(first, &a, &RETRIED, "from stdin\n", &[CRASH_AFTER]);
    assert_eq!(
        crashed.status.code(),
        Some(99),
        "{}",
        String::from_utf8_lossy(&crashed.stderr)
    );
    assert_eq!(receipts(&a).len(), 1);
    let changed = call(second, &a, &RETRIED, "changed\n", &[]);
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    let shown = &ok(second, &a, &["show", "ex-a"])[0];
    assert_eq!(shown["description"].as_str(), Some("changed\n"));
    assert_eq!(ops_by_a(second, &a), ["create", "claim", "set", "set"]);
    // the stale receipt is dropped, and the new request's goes with its attempt once it counts
    assert!(receipts(&a).is_empty(), "a receipt was kept");
}

#[test]
fn rust_recognises_an_accepted_request_python_left() {
    retried_by_the_other(&python(), &rust());
}

#[test]
fn python_recognises_an_accepted_request_rust_left() {
    retried_by_the_other(&rust(), &python());
}

#[test]
fn rust_takes_changed_content_for_a_new_request_after_python() {
    changed_content_is_a_new_request(&python(), &rust());
}

#[test]
fn python_takes_changed_content_for_a_new_request_after_rust() {
    changed_content_is_a_new_request(&rust(), &python());
}

/// `first` leaves an accepted claim whose acknowledgement was lost, and another machine takes the
/// issue over before machine a runs again: `second` recognises the retry, applies nothing, and says
/// the claim it answers for has moved on.
fn retried_after_a_takeover(first: &Path, second: &Path) {
    let sandbox = crate::sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    tracker(first, &sandbox, &a);
    let crashed = call(first, &a, &["claim", "ex-a"], "", &[CRASH_AFTER]);
    assert_eq!(
        crashed.status.code(),
        Some(99),
        "{}",
        String::from_utf8_lossy(&crashed.stderr)
    );
    ok(second, &b, &["sync"]);
    ok(second, &b, &["takeover", "ex-a", "-r", "a went quiet"]);
    let retry = call(second, &a, &["claim", "ex-a", "--json"], "", &[]);
    assert_eq!(code(&retry), "ALREADY_CLAIMED");
    let error = parse_json(&String::from_utf8_lossy(&retry.stderr)).expect("a JSON error");
    let message = error["error"]["message"].as_str().expect("a message");
    assert!(
        message.starts_with("ex-a: your claim was accepted but is now held by "),
        "{message}"
    );
    assert_eq!(
        ops_by_a(second, &a),
        ["create", "claim"],
        "the claim was applied again"
    );
    assert!(
        receipts(&a).is_empty(),
        "a recognised retry retires its receipt"
    );
}

#[test]
fn rust_answers_for_a_claim_python_left_that_moved_on() {
    retried_after_a_takeover(&python(), &rust());
}

#[test]
fn python_answers_for_a_claim_rust_left_that_moved_on() {
    retried_after_a_takeover(&rust(), &python());
}

/// Rust stops right after an accepted attempt counts here, before its pending twin is removed (the
/// point Python's hooks cannot reach); Python, run again as the same actor, recognises the request.
#[test]
fn python_recognises_a_request_rust_confirmed_before_it_stopped() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    tracker(&rust(), &sandbox, &a);
    let stop = ("BEAD_FAILPOINTS", "ownership.after-confirm=exit(97)");
    let stopped = call(&rust(), &a, &RETRIED, "from stdin\n", &[stop]);
    assert_eq!(
        stopped.status.code(),
        Some(97),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert_eq!(
        ok(&rust(), &a, &["info"])["pending_ownership"]
            .as_array()
            .map(<[Json]>::len),
        Some(2)
    );
    let retry = call(&python(), &a, &RETRIED, "from stdin\n", &[]);
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert_eq!(ops_by_a(&python(), &a), ["create", "claim", "set"]);
    assert_eq!(
        ok(&python(), &a, &["info"])["pending_ownership"],
        Json::Array(Vec::new())
    );
    assert!(
        receipts(&a).is_empty(),
        "a recognised retry retires its receipt"
    );
}
