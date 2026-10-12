//! A command repeated on an issue already as it asks, or an update that asks for nothing, writes
//! nothing and says so; a repeat that would still settle something is written as before.

use beadroll_test_support::{Json, Machine, Sandbox, parse_json};

/// Machine a with tracker `ex` and one issue: its key.
fn set_up(sandbox: &Sandbox) -> (Machine, String) {
    let a = Machine::new(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let made = a.bead(["create", "Repeated"]).unwrap();
    (a, made["id"].as_str().unwrap().to_owned())
}

/// The operations `bead log KEY` lists, in order.
fn ops(machine: &Machine, key: &str) -> Vec<String> {
    let log = machine.bead(["log", key]).unwrap();
    let rows = log.as_array().unwrap();
    rows.iter()
        .map(|row| row["op"].as_str().unwrap().to_owned())
        .collect()
}

/// Runs `bead ARGS` on `machine`, which must succeed: its standard output.
fn ok(machine: &Machine, args: &[&str]) -> String {
    let run = machine.run(args);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    run.stdout
}

#[test]
fn a_repeated_close_delete_or_reopen_writes_nothing_and_keeps_the_reason() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    let first = ok(&a, &["close", &key, "-r", "first reason"]);
    assert!(
        first.starts_with(&format!("✓ Closed {key}: Repeated")),
        "{first}"
    );
    let again = ok(&a, &["close", &key, "-r", "second reason"]);
    assert_eq!(
        again.lines().collect::<Vec<_>>(),
        [
            format!("✓ Already closed {key}: Repeated"),
            format!("{key} was already closed: the reason given is not recorded"),
        ]
    );
    assert_eq!(a.show(&key)["close_reason"].as_str(), Some("first reason"));
    let json = a.run(["close", &key, "--json"]);
    let json = parse_json(&json.stdout).expect("one JSON document");
    assert_eq!(json[0]["unchanged"].as_str(), Some("already closed"));
    assert_eq!(ops(&a, &key), ["create", "close"]);

    // once each in one command: the second sees the first
    let reopened = ok(&a, &["reopen", &key, &key]);
    assert_eq!(
        reopened.lines().collect::<Vec<_>>(),
        [
            format!("✓ Reopened {key}: Repeated"),
            format!("✓ Already open {key}: Repeated"),
        ]
    );
    ok(&a, &["delete", &key, "-r", "gone"]);
    let deleted = ok(&a, &["delete", &key]);
    assert_eq!(deleted.trim(), format!("✓ Already deleted {key}: Repeated"));
    assert_eq!(ops(&a, &key), ["create", "close", "reopen", "delete"]);
}

#[test]
fn an_update_that_asks_for_nothing_writes_nothing_and_says_so() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    let said = ok(&a, &["update", &key]);
    assert_eq!(
        said.lines().collect::<Vec<_>>(),
        [
            format!("Unchanged {key}: Repeated"),
            "nothing to update was given: name a field, --status, a label or --parent \
             (bead update --help)"
                .to_owned(),
        ]
    );
    let json = a.bead(["update", &key]).unwrap();
    assert_eq!(
        json[0]["unchanged"].as_str(),
        Some("nothing to update was given")
    );
    assert_eq!(ops(&a, &key), ["create"]);
    // anything asked is an update, as before
    let renamed = ok(&a, &["update", &key, "--title", "Renamed"]);
    assert_eq!(renamed.trim(), format!("Updated {key}: Renamed"));
    assert!(
        a.bead(["update", &key, "--title", "Renamed"]).unwrap()[0]
            .get("unchanged")
            .is_none()
    );
}

#[test]
fn a_repeated_close_still_settles_a_disputed_status() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    // b defers it, unheard, while a closes it: two statuses, the later close shown
    ok(&b, &["update", &key, "--status", "deferred", "--offline"]);
    ok(&a, &["close", &key, "-r", "by a"]);
    b.bead(["sync"]);
    a.bead(["sync"]);
    let disputed = |machine: &Machine| -> Json { machine.show(&key)["conflicts"].clone() };
    assert!(disputed(&a).get("status").is_some(), "{:?}", disputed(&a));
    assert_eq!(a.show(&key)["status"].as_str(), Some("closed"));

    // closed as shown, but disputed: the repeat is the write that settles it
    let settled = ok(&a, &["close", &key, "-r", "settled"]);
    assert!(settled.starts_with(&format!("✓ Closed {key}")), "{settled}");
    assert_eq!(a.show(&key)["close_reason"].as_str(), Some("settled"));
    assert!(disputed(&a).get("status").is_none(), "{:?}", disputed(&a));
    assert_eq!(ops(&a, &key).iter().filter(|op| *op == "close").count(), 2);
}
