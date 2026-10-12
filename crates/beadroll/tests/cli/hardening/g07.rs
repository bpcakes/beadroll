//! G07 (`docs/FINDINGS.md`): a settled receipt survived the actor's later successful local writes,
//! so a deliberate repeat of a claim was answered "Claimed (confirmed)" while nobody held the issue.
//! The first attempt stops on the crash hook `BEAD_TEST_CRASH_AFTER_PUSH`, which both binaries
//! implement: exit status 99 once the remote has taken the push.

use beadroll_test_support::{Machine, Sandbox};

/// `_g07_machine`: a machine `name` that initialised the tracker (prefix `ex`), or, with `init`
/// false, synced the one already published.
fn machine(sandbox: &Sandbox, name: &str, init: bool) -> Machine {
    let machine = Machine::new(sandbox, name);
    if init {
        machine.bead(["init", "--prefix", "ex"]);
    } else {
        machine.bead(["sync"]);
    }
    machine
}

/// Only a coordinated command drops an actor's receipts. Later local writes by that actor (close,
/// reopen, update without --claim, comments) go through without dropping them, so a later
/// deliberate repeat of the same command is answered from the old receipt and does nothing,
/// exit 0.
mod g07_receipt_survives_later_local_writes {
    use beadroll_test_support::{Json, Machine};

    use super::machine;

    /// The id of the issue a `bead create --json` made: `result["id"]`.
    fn id(created: Option<Json>) -> String {
        let created = created.expect("bead printed the issue it made");
        created["id"].as_str().expect("an id").to_owned()
    }

    /// The claim `show` reports, `None` when there is none or it is `null`.
    fn claim(issue: &Json) -> Option<&Json> {
        issue.get("claim").filter(|claim| **claim != Json::Null)
    }

    /// Runs `bead ARGS` on `machine` and asserts it exited 0.
    fn succeeds(machine: &Machine, args: &[&str]) {
        let run = machine.run(args);
        assert_eq!(run.returncode(), 0, "bead {args:?}: {}", run.stderr);
    }

    /// G07: after the actor's own close, reopen and edit went through, `claim X` claims X; it is
    /// not answered "Claimed (confirmed)" from the old receipt while nobody holds X.
    #[test]
    fn new_claim_after_close_and_reopen_claims() {
        let sandbox = crate::sandbox();
        let a = machine(&sandbox, "g07-again", true);
        let x = id(a.bead(["create", "X"]));
        a.bead(["sync"]);
        let crashed = a
            .cmd(["claim", &x])
            .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
            .run();
        assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
        succeeds(&a, &["sync"]); // the agent finds it holds X and works on it
        succeeds(&a, &["close", &x]);
        succeeds(&a, &["reopen", &x]);
        succeeds(&a, &["update", &x, "--title", "X again"]);
        let claimed = a.run(["claim", &x]);
        assert_eq!(claimed.returncode(), 0, "{}", claimed.stderr);
        assert!(
            claim(&a.show(&x)).is_some(),
            "'{}' but nobody holds {x}",
            claimed.stdout.trim()
        );
    }

    /// G07: after the actor's own reopen went through, `update X --claim --status closed` closes X
    /// again; it is not answered "Updated" from the old receipt while X stays open.
    #[test]
    fn new_claiming_close_after_reopen_closes() {
        let sandbox = crate::sandbox();
        let a = machine(&sandbox, "g07-again-close", true);
        let x = id(a.bead(["create", "X"]));
        a.bead(["sync"]);
        let crashed = a
            .cmd(["update", &x, "--claim", "--status", "closed"])
            .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
            .run();
        assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
        succeeds(&a, &["sync"]);
        succeeds(&a, &["reopen", &x]);
        let again = a.run(["update", &x, "--claim", "--status", "closed"]);
        assert_eq!(again.returncode(), 0, "{}", again.stderr);
        assert_eq!(
            a.show(&x).get("status"),
            Some(&Json::from("closed")),
            "'{}' but {x} is still open",
            again.stdout.trim()
        );
    }

    /// G07: a comment made before anyone synced (the attempt still pending) went through after the
    /// crash, so once sync has settled the attempt and the claim has ended, `claim X` claims X.
    #[test]
    fn write_made_while_the_attempt_was_pending_also_moves_the_actor_on() {
        let sandbox = crate::sandbox();
        let a = machine(&sandbox, "g07-pending-write", true);
        let x = id(a.bead(["create", "X"]));
        a.bead(["sync"]);
        let crashed = a
            .cmd(["claim", &x])
            .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
            .run();
        assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
        succeeds(&a, &["comments", "add", &x, "work continues"]);
        succeeds(&a, &["sync"]); // settles the attempt: a holds X
        let b = machine(&sandbox, "g07-pending-write-b", false);
        succeeds(&b, &["takeover", &x, "--reason", "a went quiet"]);
        succeeds(&b, &["release", &x]);
        succeeds(&a, &["sync"]);
        let shown = a.show(&x);
        assert!(claim(&shown).is_none(), "{shown:?}");
        let claimed = a.run(["claim", &x]);
        assert_eq!(claimed.returncode(), 0, "{}", claimed.stderr);
        assert!(
            claim(&a.show(&x)).is_some(),
            "'{}' but nobody holds {x}",
            claimed.stdout.trim()
        );
    }
}
