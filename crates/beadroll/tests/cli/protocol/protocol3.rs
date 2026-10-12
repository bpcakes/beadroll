//! `Protocol3`, black-box: a fence the remote refuses settles nothing, so the attempt it was to
//! settle stays pending and counts for nothing until one lands; and a claiming update killed while
//! it confirms the attempt the remote accepted leaves all of it or none, never part (the third
//! independent review, 2026-10-06). The kill is the Rust `bead`'s `atomic-write:claims/`
//! failpoint. Related in-process tests are in `beadroll-sync`
//! (`tests/sync/protocol/protocol3.rs`).

use std::os::unix::fs::PermissionsExt as _;

use beadroll_store::{glob_ending, read_attempt};
use beadroll_test_support::{Json, Machine, Sandbox, code};

/// Machine a's tracker with one issue, "before", synced to machine b.
struct Protocol3 {
    sandbox: Sandbox,
    a: Machine,
    b: Machine,
    key: String,
}

fn setup() -> Protocol3 {
    setup_with(Machine::new)
}

/// Sets up machines made by `make`: `Machine::new`, or `crate::rust_only` for a test that
/// injects its fault through a failpoint.
fn setup_with(make: fn(&Sandbox, &str) -> Machine) -> Protocol3 {
    let sandbox = crate::sandbox();
    let a = make(&sandbox, "protocol3-a");
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "before"]).unwrap();
    let key = created["id"]
        .as_str()
        .expect("the new issue's id")
        .to_owned();
    a.bead(["sync"]);
    let b = make(&sandbox, "protocol3-b");
    b.bead(["sync"]);
    Protocol3 { sandbox, a, b, key }
}

/// The member `key` of `value`; absence fails the test.
fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}

/// What a reader sees of a claiming update on `issue`: whether it is claimed, its title and its
/// labels (an empty vector when it has none).
fn seen(issue: &Json) -> (bool, String, Vec<String>) {
    let title = member(issue, "title").as_str().expect("a title");
    let labels = issue.get("labels").and_then(Json::as_array);
    let labels = labels.unwrap_or_default().iter();
    let labels = labels.map(|label| label.as_str().expect("a label").to_owned());
    (
        issue.get("claim").is_some(),
        title.to_owned(),
        labels.collect(),
    )
}

impl Protocol3 {
    /// The ids of the events of machine a's unsettled attempts, as bead reads its `pending/`: an
    /// attempt is one file holding every event that lands with it.
    fn pending_ids(&self) -> Vec<String> {
        let attempts = glob_ending(&self.a.store().join("pending"), ".json");
        attempts
            .iter()
            .flat_map(|path| read_attempt(path, None))
            .map(|event| event["id"].as_str().expect("an event id").to_owned())
            .collect()
    }
}

/// A claim stopped before its push leaves its attempt pending. While the remote's hook refuses
/// every push, the fence a sync pushes to settle the attempt is refused (REMOTE_REFUSED): the
/// attempt stays pending and counts for nothing. Once the remote takes pushes again, a sync
/// settles it, and the claim never counted.
#[test]
fn rejected_fences_leave_attempt_pending_and_excluded() {
    let s = setup();
    let crash =
        s.a.cmd(["claim", &s.key])
            .env("BEAD_TEST_CRASH_BEFORE_PUSH", "1")
            .run();
    assert_eq!(crash.returncode(), 98, "{}", crash.stderr);
    let before = s.pending_ids();
    assert_eq!(before.len(), 1);
    let hook = s.sandbox.origin().join("hooks").join("pre-receive");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Refusal ends sync in its first round, before any backoff.
    let refused = s.a.run(["sync", "--json"]);
    assert_eq!(refused.returncode(), 1, "{}", refused.stdout);
    assert_eq!(code(&refused), "REMOTE_REFUSED");
    assert_eq!(s.pending_ids(), before);
    assert!(s.a.show(&s.key).get("claim").is_none());
    assert_eq!(s.a.show(&s.key)["title"].as_str(), Some("before"));
    std::fs::remove_file(&hook).unwrap();
    s.a.bead(["sync"]);
    assert_eq!(s.pending_ids(), Vec::<String>::new());
    assert!(s.a.show(&s.key).get("claim").is_none());
}

/// The remote accepts all files; SIGKILL interrupts local confirmation. Inject
/// `atomic-write:claims/@always=sigkill` after writing the temporary attempt file and before
/// rename. Exit by SIGKILL proves confirmation was reached; retry must settle the remaining
/// files.
#[test]
fn kill_during_local_confirmation_does_not_leave_partial_atomic_update() {
    let s = setup_with(crate::rust_only);
    let update = [
        "update",
        &s.key,
        "--claim",
        "--title",
        "after",
        "--add-label",
        "accepted",
        "--json",
    ];
    let process =
        s.a.cmd(update)
            .env("BEAD_FAILPOINTS", "atomic-write:claims/@always=sigkill")
            .run();
    assert_eq!(
        process.returncode(),
        -9,
        "confirmation fault boundary was not reached: {}",
        process.stderr
    );
    let partial = s.a.show(&s.key);
    s.b.bead(["sync"]);
    let remote = s.b.show(&s.key);
    s.a.bead(["sync"]);
    let recovered = s.a.show(&s.key);
    assert_eq!(recovered["title"].as_str(), Some("after"));
    assert_eq!(
        member(&recovered, "labels"),
        &Json::Array(vec![Json::from("accepted")])
    );
    assert_eq!(member(&recovered, "claim"), member(&remote, "claim"));
    let none = (false, "before".to_owned(), Vec::new());
    let all = (true, "after".to_owned(), vec!["accepted".to_owned()]);
    let partial = seen(&partial);
    assert!(
        partial == none || partial == all,
        "SIGKILL leaves accepted claim visible while other events of the transaction remain only \
         pending: {partial:?}"
    );
    assert_eq!(
        s.a.bead(["info"]).unwrap()["integrity"],
        Json::Array(Vec::new()),
        "a half-written attempt file must never be read as events"
    );
}
