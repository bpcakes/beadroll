//! An accepted request is never applied twice when retried by the same actor on the same home,
//! including after sync, a later failure, takeover or a stop after confirmation. Changed request
//! content is a new request. `parity::receipts` separately pins the signature bytes.

use std::process::Output;

use beadroll_test_support::{Json, Machine, parse_json};

use super::receipts::{CRASH_AFTER, call, ok, receipts, tracker};

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
fn ops_by_a(a: &Machine) -> Vec<String> {
    let log = ok(a, &["log", "ex-a", "--limit", "0"]);
    log.as_array()
        .expect("a list")
        .iter()
        .filter(|row| row["thread"].as_str() == Some("a-thread-0001"))
        .map(|row| row["op"].as_str().expect("an op").to_owned())
        .collect()
}

/// How the first run leaves the store when the retry comes.
#[derive(Clone, Copy, Debug)]
enum Switch {
    /// It syncs first: no ownership is pending, and the accepted attempt's receipt is kept.
    AfterSync,
    /// It stops with the accepted attempt still pending, for the retry to settle.
    Outstanding,
}

/// Machine a leaves an accepted claiming update whose acknowledgement was lost; machine b edits the
/// issue after it; a syncs (or, `Outstanding`, leaves the attempt pending). Then a runs again as
/// the same actor: another actor's identical command is refused, and the retry is recognised and
/// retired without applying anything again, so the newer edit stands, the claim stays the actor's,
/// and nothing is left pending or damaged.
fn retried(switch: Switch) {
    let sandbox = crate::sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    tracker(&sandbox, &a);
    let crashed = call(&a, &RETRIED, "from stdin\n", &[CRASH_AFTER]);
    let said = String::from_utf8_lossy(&crashed.stderr);
    assert_eq!(crashed.status.code(), Some(99), "{said}");
    ok(&b, &["sync"]);
    ok(&b, &["update", "ex-a", "--title", "newer title"]);
    ok(&b, &["sync"]);
    if let Switch::AfterSync = switch {
        ok(&a, &["sync"]);
    }
    let info = ok(&a, &["info"]);
    let left = info["pending_ownership"].as_array().map(<[Json]>::len);
    match switch {
        Switch::AfterSync => assert_eq!(left, Some(0), "{switch:?}: sync settles the attempt"),
        Switch::Outstanding => assert!(
            left.is_some_and(|n| n > 0),
            "{switch:?}: the attempt is still pending"
        ),
    }
    let kept = "the accepted attempt's receipt is kept";
    assert_eq!(receipts(&a).len(), 1, "{switch:?}: {kept}");

    let mut other = RETRIED.to_vec();
    other.push("--json");
    let elsewhere = [("BEAD_THREAD", "other-thread")];
    let refused = call(&a, &other, "from stdin\n", &elsewhere);
    assert_eq!(code(&refused), "ALREADY_CLAIMED");
    // a later command of the actor that fails leaves the receipt in place
    let failed = call(&a, &["claim", "ex-zzz", "--json"], "", &[]);
    assert_eq!(code(&failed), "ISSUE_NOT_FOUND");
    assert_eq!(receipts(&a).len(), 1);

    let retry = call(&a, &RETRIED, "from stdin\n", &[]);
    let said = String::from_utf8_lossy(&retry.stderr);
    assert!(retry.status.success(), "{said}");
    let shown = &ok(&a, &["show", "ex-a"])[0];
    assert_eq!(shown["title"].as_str(), Some("newer title"));
    assert_eq!(shown["description"].as_str(), Some("from stdin\n"));
    assert_eq!(shown["claim"]["thread"].as_str(), Some("a-thread-0001"));
    let again = "the request was applied again";
    assert_eq!(ops_by_a(&a), ["create", "claim", "set"], "{again}");
    let retired = "a recognised retry retires its receipt";
    assert!(receipts(&a).is_empty(), "{retired}");
    let info = ok(&a, &["info"]);
    let none = Json::Array(Vec::new());
    assert_eq!(info["pending_ownership"], none, "{switch:?}");
    assert_eq!(info["integrity"], none, "{switch:?}");
}

#[test]
fn an_accepted_request_is_recognised_after_a_sync() {
    retried(Switch::AfterSync);
}

#[test]
fn an_outstanding_request_is_settled_and_recognised() {
    retried(Switch::Outstanding);
}

/// The same command with other content on standard input is another request: it is applied, and
/// the receipt the first run left is dropped as stale once it has gone through, as is its own.
#[test]
fn changed_content_is_a_new_request() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    tracker(&sandbox, &a);
    let crashed = call(&a, &RETRIED, "from stdin\n", &[CRASH_AFTER]);
    let said = String::from_utf8_lossy(&crashed.stderr);
    assert_eq!(crashed.status.code(), Some(99), "{said}");
    assert_eq!(receipts(&a).len(), 1);
    let changed = call(&a, &RETRIED, "changed\n", &[]);
    let said = String::from_utf8_lossy(&changed.stderr);
    assert!(changed.status.success(), "{said}");
    let shown = &ok(&a, &["show", "ex-a"])[0];
    assert_eq!(shown["description"].as_str(), Some("changed\n"));
    assert_eq!(ops_by_a(&a), ["create", "claim", "set", "set"]);
    // the stale receipt is dropped, and the new request's goes with its attempt once it counts
    assert!(receipts(&a).is_empty(), "a receipt was kept");
}

/// Machine a leaves an accepted claim whose acknowledgement was lost, and machine b takes the
/// issue over before a runs again: a's retry is recognised, applies nothing, and says the claim it
/// answers for has moved on.
#[test]
fn a_retry_answers_for_a_claim_that_moved_on() {
    let sandbox = crate::sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    tracker(&sandbox, &a);
    let crashed = call(&a, &["claim", "ex-a"], "", &[CRASH_AFTER]);
    let said = String::from_utf8_lossy(&crashed.stderr);
    assert_eq!(crashed.status.code(), Some(99), "{said}");
    ok(&b, &["sync"]);
    ok(&b, &["takeover", "ex-a", "-r", "a went quiet"]);
    let retry = call(&a, &["claim", "ex-a", "--json"], "", &[]);
    assert_eq!(code(&retry), "ALREADY_CLAIMED");
    let error = parse_json(&String::from_utf8_lossy(&retry.stderr)).expect("a JSON error");
    let message = error["error"]["message"].as_str().expect("a message");
    let moved = "ex-a: your claim was accepted but is now held by ";
    assert!(message.starts_with(moved), "{message}");
    let again = "the claim was applied again";
    assert_eq!(ops_by_a(&a), ["create", "claim"], "{again}");
    let retired = "a recognised retry retires its receipt";
    assert!(receipts(&a).is_empty(), "{retired}");
}

/// Stop after confirmation but before pending-twin removal, then retry as the same actor. The
/// request must be recognised. This requires a failpoint from the current build.
#[test]
fn a_request_confirmed_before_a_stop_is_recognised() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    tracker(&sandbox, &a);
    let stop = ("BEAD_FAILPOINTS", "ownership.after-confirm=exit(97)");
    let stopped = call(&a, &RETRIED, "from stdin\n", &[stop]);
    let said = String::from_utf8_lossy(&stopped.stderr);
    assert_eq!(stopped.status.code(), Some(97), "{said}");
    let pending = ok(&a, &["info"])["pending_ownership"]
        .as_array()
        .map(<[Json]>::len);
    assert_eq!(pending, Some(2));
    let retry = call(&a, &RETRIED, "from stdin\n", &[]);
    let said = String::from_utf8_lossy(&retry.stderr);
    assert!(retry.status.success(), "{said}");
    assert_eq!(ops_by_a(&a), ["create", "claim", "set"]);
    let none = Json::Array(Vec::new());
    assert_eq!(ok(&a, &["info"])["pending_ownership"], none);
    let retired = "a recognised retry retires its receipt";
    assert!(receipts(&a).is_empty(), "{retired}");
}
