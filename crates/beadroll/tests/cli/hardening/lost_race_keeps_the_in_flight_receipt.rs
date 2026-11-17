//! G09: after settling a receipt, a claim whose push loses a race must retry without corrupting
//! the in-flight receipt. This `rust_only` test pauses at `claim.before-push` while another
//! machine publishes (`docs/FINDINGS.md`).
//!
//! `for receipt in stale:` (bead 1910) rebinds `receipt`, the in-flight (events, result) tuple, to
//! a Path. When the push is then refused because another machine published first, the next round
//! evaluates `receipt[0]` on a PosixPath: TypeError, a traceback instead of another round.

use beadroll_store::glob_ending;
use beadroll_test_support::{Gate, Machine, Sandbox, beside, printed_id};

use crate::hardening::TrackerSetup;

/// A machine `name` running the Rust `bead`, that initialised the tracker (prefix
/// `ex`), or, with `init` false, synced the one already published.
fn machine(sandbox: &Sandbox, name: &str, init: TrackerSetup) -> Machine {
    let machine = crate::rust_only(sandbox, name);
    if init == TrackerSetup::Init {
        machine.bead(["init", "--prefix", "ex"]);
    } else {
        machine.bead(["sync"]);
    }
    machine
}

/// G09: after an earlier lost acknowledgement leaves a settled receipt, a claim losing its
/// first push race must retry successfully. Pause at the first claim.before-push, between
/// fetch and push, while machine B creates an issue and syncs.
#[test]
fn ordinary_contention_after_a_settled_receipt_retries_the_claim() {
    let sandbox = crate::sandbox();
    let a = machine(&sandbox, "g09-contend", TrackerSetup::Init);
    let x = printed_id(a.bead(["create", "X"]));
    let y = printed_id(a.bead(["create", "Y"]));
    a.bead(["sync"]);
    let b = machine(&sandbox, "g09-contend-b", TrackerSetup::Sync);
    let lost = a
        .cmd(["claim", &x])
        .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
        .run();
    assert_eq!(lost.returncode(), 99, "{}", lost.stderr);
    // the accepted claim is found; its receipt is settled
    assert_eq!(a.run(["sync"]).returncode(), 0);
    let receipts = glob_ending(&a.store().join("pending"), ".command");
    assert_eq!(receipts.len(), 1, "{receipts:?}");
    let contention = Gate::new(&sandbox, "g09-before-claim-push");
    let failpoints = format!("claim.before-push={}", contention.action());
    let (claimed, contended) = beside(
        || {
            let claim = a.cmd(["claim", &y, "--json"]);
            claim.env("BEAD_FAILPOINTS", &failpoints).run()
        },
        |finished| {
            contention.meet(finished, || {
                b.bead(["create", "B's unrelated work"]);
                // machine B publishes between A's fetch and A's push
                b.bead(["sync"]);
            })
        },
    );
    assert!(contended.is_some(), "the contention was not injected");
    assert!(
        !claimed.crashed(),
        "claim crashed after an ordinary refused push: {}",
        claimed.stderr
    );
    assert_eq!(claimed.returncode(), 0, "{}", claimed.stderr);
    assert_eq!(
        a.show(&y)["claim"]["thread"].as_str(),
        Some("g09-contend-thread-0001")
    );
}
