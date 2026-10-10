//! `Protocol3`, black-box: a fence the remote refuses settles nothing, so the attempt it was to
//! settle stays pending and counts for nothing until one lands (the third independent review,
//! 2026-10-06). The class's in-process tests are in `beadroll-sync`
//! (`tests/sync/protocol/protocol3.rs`).

use std::os::unix::fs::PermissionsExt as _;

use beadroll_store::{glob_ending, read_attempt};
use beadroll_test_support::{Machine, Sandbox, code};

/// Machine a's tracker with one issue, "before", synced to machine b: the class's `setUp`, less
/// the in-process store its other tests open.
struct Protocol3 {
    sandbox: Sandbox,
    a: Machine,
    key: String,
}

fn setup() -> Protocol3 {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "protocol3-a");
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "before"]).unwrap();
    let key = created["id"]
        .as_str()
        .expect("the new issue's id")
        .to_owned();
    a.bead(["sync"]);
    // machine b only syncs here: no test of this file reads it again
    let b = Machine::new(&sandbox, "protocol3-b");
    b.bead(["sync"]);
    Protocol3 { sandbox, a, key }
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
    // Python synchronises in process here; the refusal ends the sync in its first round, so no
    // wait between rounds is ever reached
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
