//! G09 (`docs/FINDINGS.md`): `for receipt in stale:` rebound the in-flight receipt, so a claim
//! whose push lost an ordinary race after a settled receipt crashed with `TypeError` instead of
//! trying another round. The `TypeError` is Python's own; the property, another round, stays. The
//! race needs another machine to publish between the claim's fetch and its push, which the Python
//! test arranges by patching `Link.publish` in process: a `rust_only` test, whose `claim` stops at
//! the `claim.before-push` failpoint while the other machine publishes.

use beadroll_test_support::{Machine, Sandbox};

/// `_g09_machine`: a machine `name` running the Rust `bead`, that initialised the tracker (prefix
/// `ex`), or, with `init` false, synced the one already published.
fn machine(sandbox: &Sandbox, name: &str, init: bool) -> Machine {
    let machine = crate::rust_only(sandbox, name);
    if init {
        machine.bead(["init", "--prefix", "ex"]);
    } else {
        machine.bead(["sync"]);
    }
    machine
}

/// `for receipt in stale:` (bead 1910) rebinds `receipt`, the in-flight (events, result) tuple, to
/// a Path. When the push is then refused because another machine published first, the next round
/// evaluates `receipt[0]` on a PosixPath: TypeError, a traceback instead of another round.
mod g09_receipt_loop_variable_shadows_the_in_flight_receipt {
    use beadroll_store::glob_ending;
    use beadroll_test_support::Json;

    use super::machine;
    use crate::hardening::gate::{Gate, beside};

    /// The id of the issue a `--json` result is: `result["id"]`.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        result["id"].as_str().expect("an id").to_owned()
    }

    /// G09: with a settled receipt left by an earlier lost acknowledgement, a claim whose first
    /// push loses an ordinary race retries and succeeds instead of crashing with TypeError.
    ///
    /// Replaces `G09_ReceiptLoopVariableShadowsTheInFlightReceipt`'s
    /// `test_ordinary_contention_after_a_settled_receipt_retries_the_claim`, which patches
    /// `Link.publish` so that the first publish of a claim first has machine B create an issue and
    /// sync; here `claim.before-push=pause-until(...)` stops the claim at its first hit, between
    /// the claim's fetch and its push, while B does the same.
    #[test]
    fn ordinary_contention_after_a_settled_receipt_retries_the_claim() {
        let sandbox = crate::sandbox();
        let a = machine(&sandbox, "g09-contend", true);
        let x = id(a.bead(["create", "X"]));
        let y = id(a.bead(["create", "Y"]));
        a.bead(["sync"]);
        let b = machine(&sandbox, "g09-contend-b", false);
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
}
