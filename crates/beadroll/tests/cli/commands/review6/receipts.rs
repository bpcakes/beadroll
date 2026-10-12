//! F01, F02 and a verified property: the retry of an accepted command is recognised by its receipt
//! (`pending/*.command`), whatever failed or ran in between, whatever the locale, and whatever
//! standard input held. The first attempt stops on the crash hook `BEAD_TEST_CRASH_AFTER_PUSH`,
//! which both binaries implement: exit status 99 once the remote has taken the push.

use beadroll_test_support::{Invocation, Machine};

use super::{count, glob, id, machine, ops};

/// `bead ARGS` from `machine`, stopped by the crash hook right after its push was accepted.
fn crashing_after_push<'m>(machine: &'m Machine, args: &[&str]) -> Invocation<'m> {
    machine.cmd(args).env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
}

/// How many `.command` receipts `machine`'s store keeps in `pending/`.
fn receipts(machine: &Machine) -> usize {
    glob(&machine.store().join("pending"), ".command").len()
}

/// F01: a failed command must not make a retry of an accepted command look new.
#[test]
fn f01_failed_command_does_not_forget_an_accepted_retry() {
    let sandbox = crate::sandbox();
    let owner = machine(&sandbox, "f01");
    let key = id(owner.bead(["create", "close once"]));
    owner.bead(["sync"]);
    let close = ["update", &key, "--claim", "--status", "closed", "--json"];
    let crashed = crashing_after_push(&owner, &close).run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    let missed = owner.run(["claim", "missing-id", "--json"]);
    assert_ne!(missed.returncode(), 0, "{}", missed.stdout);
    assert_eq!(receipts(&owner), 1);
    let retry = owner.run(close);
    assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    assert_eq!(count(&ops(&owner, &key), "close"), 1);

    let parent = machine(&sandbox, "f01-parent");
    let first = id(parent.bead(["create", "first"]));
    let second = id(parent.bead(["create", "second"]));
    let child = id(parent.bead(["create", "child", "--parent", &first]));
    parent.bead(["sync"]);
    let move_child = ["update", &child, "--claim", "--parent", &second, "--json"];
    let crashed = crashing_after_push(&parent, &move_child).run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    parent.run(["claim", "missing-id", "--json"]);
    let retry = parent.run(move_child);
    assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    assert_eq!(
        parent.show(&child).get("parent").and_then(|p| p.as_str()),
        Some(second.as_str())
    );
    assert_eq!(count(&ops(&parent, &child), "dep_remove"), 1);
}

/// F02: a retry decodes the same file as the same request, whatever the locale.
#[test]
fn f02_locale_retry_keeps_the_accepted_description() {
    let sandbox = crate::sandbox();
    let body = sandbox.root().join("cafe.txt");
    std::fs::write(&body, "café body\n".as_bytes()).unwrap();
    let body = body.to_str().unwrap();
    let latin = machine(&sandbox, "f02-latin");
    let key = id(latin.bead(["create", "locale"]));
    latin.bead(["sync"]);
    let describe = [
        "update",
        &key,
        "--claim",
        "--description-file",
        body,
        "--json",
    ];
    let crashed = crashing_after_push(&latin, &describe).run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    let retry = latin
        .cmd(describe)
        .env("LC_ALL", "en_US.ISO8859-1")
        .env("LANG", "en_US.ISO8859-1")
        .env("PYTHONUTF8", "0")
        .env("PYTHONCOERCECLOCALE", "0")
        .run();
    assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    assert_eq!(
        latin.show(&key).get("description").and_then(|d| d.as_str()),
        Some("café body\n")
    );
    assert_eq!(count(&ops(&latin, &key), "set"), 1);

    let sea = machine(&sandbox, "f02-c");
    let key = id(sea.bead(["create", "locale c"]));
    sea.bead(["sync"]);
    let describe = [
        "update",
        &key,
        "--claim",
        "--description-file",
        body,
        "--json",
    ];
    let crashed = crashing_after_push(&sea, &describe).run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    let retry = sea
        .cmd(describe)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("PYTHONUTF8", "0")
        .env("PYTHONCOERCECLOCALE", "0")
        .run();
    assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    assert!(!retry.crashed(), "{}", retry.stderr);
    assert_eq!(
        sea.show(&key).get("description").and_then(|d| d.as_str()),
        Some("café body\n")
    );
}

/// Verified (no finding): another thread, a sync, changed stdin and `--json` do not confuse a
/// receipt.
#[test]
fn receipt_identity_holds_for_actor_sync_stdin_and_json() {
    let sandbox = crate::sandbox();
    let m = machine(&sandbox, "receipt-ok");
    let key = id(m.bead(["create", "receipt"]));
    m.bead(["sync"]);
    let body = sandbox.root().join("body.txt");
    std::fs::write(&body, "first body").unwrap();
    let path = body.to_str().unwrap();
    let describe = [
        "update",
        &key,
        "--claim",
        "--description-file",
        path,
        "--json",
    ];
    let crashed = crashing_after_push(&m, &describe).run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    let other = m
        .cmd(["claim", "missing-id", "--json"])
        .thread("other-thread")
        .run();
    assert_ne!(other.returncode(), 0, "{}", other.stdout);
    assert_eq!(receipts(&m), 1);
    let synced = m.run(["sync", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    assert_eq!(receipts(&m), 1);
    std::fs::write(&body, "second body").unwrap();
    let retry = m.run(describe);
    assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    assert_eq!(m.show(&key)["description"].as_str(), Some("second body"));

    let piped = machine(&sandbox, "receipt-stdin");
    let key = id(piped.bead(["create", "stdin"]));
    piped.bead(["sync"]);
    let from_stdin = [
        "update",
        &key,
        "--claim",
        "--description-file",
        "-",
        "--json",
    ];
    let crashed = crashing_after_push(&piped, &from_stdin)
        .stdin("stdin one")
        .run();
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    let retry = piped.cmd(from_stdin).stdin("stdin two").run();
    assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    assert_eq!(piped.show(&key)["description"].as_str(), Some("stdin two"));

    let flagged = machine(&sandbox, "receipt-json");
    let key = id(flagged.bead(["create", "json flag"]));
    flagged.bead(["sync"]);
    let crashed =
        crashing_after_push(&flagged, &["update", &key, "--claim", "--title", "once"]).run();
    let retry = flagged.run(["update", &key, "--claim", "--title", "once", "--json"]);
    assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
    assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    assert_eq!(count(&ops(&flagged, &key), "set"), 1);
}
