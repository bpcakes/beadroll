//! G10 (`docs/FINDINGS.md`): a recognised retry reported "Claimed (confirmed)" or "Updated" while
//! another agent held the issue, and a takeover retry crashed with `KeyError`. The first attempt
//! stops on the crash hook `BEAD_TEST_CRASH_AFTER_PUSH`, which both binaries implement: exit
//! status 99 once the remote has taken the push.

use beadroll_test_support::{Machine, Sandbox};

use crate::hardening::TrackerSetup;

/// `_g10_machine`: a machine `name` that initialised the tracker (prefix `ex`), or, with `init`
/// false, synced the one already published.
fn machine(sandbox: &Sandbox, name: &str, init: TrackerSetup) -> Machine {
    let init = matches!(init, TrackerSetup::Init);
    let machine = Machine::new(sandbox, name);
    if init {
        machine.bead(["init", "--prefix", "ex"]);
    } else {
        machine.bead(["sync"]);
    }
    machine
}

/// A recognised retry (settle_commands -> done) returns the original mode without looking at the
/// claim as it is now: `claim X` answers "Claimed X (confirmed)", exit 0, while another agent holds
/// X; `update X --claim` answers "Updated"; `takeover X` crashes with KeyError when the claim has
/// since ended.
mod g10_recognised_retry_reports_a_claim_that_moved_on {
    use crate::hardening::TrackerSetup;

    use beadroll_test_support::{Invocation, Json, Machine};

    use super::machine;

    /// The id of the issue a `bead create --json` made: `result["id"]`.
    fn id(created: Option<Json>) -> String {
        let created = created.expect("bead printed the issue it made");
        created["id"].as_str().expect("an id").to_owned()
    }

    /// `bead ARGS` from `machine`, stopped by the crash hook right after its push was accepted.
    fn crashing_after_push<'m>(machine: &'m Machine, args: &[&str]) -> Invocation<'m> {
        machine.cmd(args).env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
    }

    /// Who holds the claim on `issue`, as `show` prints it: `issue["claim"]["owner"]`, which must
    /// be there.
    fn owner(issue: &Json) -> String {
        let claim = issue
            .get("claim")
            .unwrap_or_else(|| panic!("no claim in {issue:?}"));
        let owner = claim
            .get("owner")
            .unwrap_or_else(|| panic!("no owner in {claim:?}"));
        owner.as_str().expect("an owner").to_owned()
    }

    /// G10: a retried `claim X` whose accepted claim was since taken over by B does not answer
    /// "Claimed X (confirmed)" with exit 0.
    #[test]
    fn claim_retry_after_a_takeover_does_not_say_claimed() {
        let sandbox = crate::sandbox();
        let a = machine(&sandbox, "g10-moved", TrackerSetup::Init);
        let x = id(a.bead(["create", "X"]));
        a.bead(["sync"]);
        let b = machine(&sandbox, "g10-moved-b", TrackerSetup::Sync);
        let crashed = crashing_after_push(&a, &["claim", &x]).run();
        assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
        let taken = b.run(["takeover", &x, "--reason", "a looked dead"]);
        assert_eq!(taken.returncode(), 0, "{}", taken.stderr);
        let retry = a.run(["claim", &x]);
        let owner = owner(&a.show(&x));
        assert!(owner.starts_with("codex:g10-moved-b-thread"), "{owner}");
        let said: String = retry.stdout.trim().chars().take(7).collect();
        assert_ne!(
            (retry.returncode(), said.as_str()),
            (0, "Claimed"),
            "told the agent it holds {x}, which {owner} holds: {}",
            retry.stdout.trim()
        );
    }

    /// G10: a retried `update X --claim --add-label mine` whose claim was since taken over by B
    /// does not exit 0 as if A held X.
    #[test]
    fn claiming_update_retry_after_a_takeover_does_not_succeed() {
        let sandbox = crate::sandbox();
        let a = machine(&sandbox, "g10-update", TrackerSetup::Init);
        let x = id(a.bead(["create", "X"]));
        a.bead(["sync"]);
        let b = machine(&sandbox, "g10-update-b", TrackerSetup::Sync);
        let label = ["update", &x, "--claim", "--add-label", "mine"];
        let crashed = crashing_after_push(&a, &label).run();
        assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
        let taken = b.run(["takeover", &x, "--reason", "a looked dead"]);
        assert_eq!(taken.returncode(), 0, "{}", taken.stderr);
        let retry = a.run(label);
        let owner = owner(&a.show(&x));
        assert!(owner.starts_with("codex:g10-update-b-thread"), "{owner}");
        assert_ne!(
            retry.returncode(),
            0,
            "claiming update exited 0 while {owner} holds {x}: {}",
            retry.stdout.trim()
        );
    }

    /// G10: a retried `takeover X` whose accepted takeover has since ended (B closed X with
    /// --force) answers with an error, not a KeyError traceback.
    #[test]
    fn takeover_retry_after_the_claim_ended_is_not_a_traceback() {
        let sandbox = crate::sandbox();
        let a = machine(&sandbox, "g10-ended", TrackerSetup::Init);
        let x = id(a.bead(["create", "X"]));
        a.bead(["sync"]);
        let b = machine(&sandbox, "g10-ended-b", TrackerSetup::Sync);
        b.bead(["claim", &x]);
        a.bead(["sync"]);
        let crashed = crashing_after_push(&a, &["takeover", &x, "--reason", "idle"]).run();
        assert_eq!(crashed.returncode(), 99, "{}", crashed.stderr);
        b.bead(["sync"]);
        let closed = b.run(["close", &x, "--force"]);
        assert_eq!(closed.returncode(), 0, "{}", closed.stderr);
        let retry = a.run(["takeover", &x, "--reason", "idle"]);
        assert!(!retry.crashed(), "{}", retry.stderr);
        assert_ne!(retry.returncode(), 0, "{}", retry.stdout);
    }
}
