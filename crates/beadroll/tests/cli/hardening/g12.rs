//! G12: retrying an owner's close, delete or `update --status closed` after a crash before
//! publication must publish the claim's end. These `rust_only` tests inject
//! `push.before=exit(98)`, which stops before the first push, including one from `publish_soon`.

/// G12: the owner's close/delete records the end of its claim locally, then publishes it at once
/// (publish_soon). If the process dies before that push, the retry finds no claim held any more,
/// so `ended` stays False: it answers success and never publishes; other machines keep seeing the
/// issue claimed until some later, unrelated sync.
mod g12_retried_end_of_claim_is_published {
    use beadroll_test_support::{Machine, Sandbox};

    /// Stops before the first push with exit status 98, leaving the writes local and unpublished.
    const BEFORE_ANY_PUSH: &str = "push.before=exit(98)";

    /// One test's world: machines a and b, and the issue a holds.
    struct SetUp {
        // removed when the test ends
        _sandbox: Sandbox,
        a: Machine,
        b: Machine,
        key: String,
    }

    /// The class's `setUp`: machine a publishes its tracker (prefix `gc`) with the issue `work`,
    /// which it claims once machine b has synced; b syncs again and sees the claim.
    fn set_up() -> SetUp {
        let sandbox = crate::sandbox();
        let (a, b) = (
            crate::rust_only(&sandbox, "a"),
            crate::rust_only(&sandbox, "b"),
        );
        a.bead(["init", "--prefix", "gc"]);
        let created = a.bead(["create", "work"]).expect("the new issue");
        let key = created["id"].as_str().expect("an id").to_owned();
        a.bead(["sync"]);
        b.bead(["sync"]);
        a.bead(["claim", &key]);
        b.bead(["sync"]);
        assert!(
            b.show(&key).get("claim").is_some(),
            "b does not see the claim"
        );
        SetUp {
            _sandbox: sandbox,
            a,
            b,
            key,
        }
    }

    /// `_g12_seen`: (status, owner) of the issue `key` as `machine` sees it.
    fn seen(machine: &Machine, key: &str) -> (String, Option<String>) {
        let issue = machine.show(key);
        let status = issue["status"].as_str().expect("a status").to_owned();
        let owner = issue["claim"]["owner"].as_str().map(str::to_owned);
        (status, owner)
    }

    /// `crash_then_retry`: `bead COMMAND KEY REST... --json` on a, stopped right before its first
    /// push; b sees nothing of it; then the same command again, which must succeed. What b then
    /// sees of the issue.
    fn crash_then_retry(command: &str, rest: &[&str]) -> (String, Option<String>) {
        let s = set_up();
        let argv: Vec<&str> = [command, s.key.as_str()]
            .into_iter()
            .chain(rest.iter().copied())
            .chain(["--json"])
            .collect();
        let crashed = s.a.cmd(&argv).env("BEAD_FAILPOINTS", BEFORE_ANY_PUSH).run();
        assert_eq!(
            crashed.returncode(),
            98,
            "the failpoint did not stop bead before the push: {}",
            crashed.stderr
        );
        s.b.bead(["sync"]);
        assert_eq!(
            seen(&s.b, &s.key).0,
            "in_progress",
            "the crashed run published after all"
        );
        let retry = s.a.run(&argv);
        assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
        s.b.bead(["sync"]);
        seen(&s.b, &s.key)
    }

    /// The message of a retried end of claim that was never published.
    fn unpublished(command: &str, seen: &(String, Option<String>)) -> String {
        format!(
            "the retried {command} answered success, yet the other machine still sees the issue \
             {} and claimed by {:?}: the end of the claim was never published",
            seen.0, seen.1
        )
    }

    /// The `(status, no owner)` a published end of claim leaves.
    fn ended(status: &str) -> (String, Option<String>) {
        (status.to_owned(), None)
    }

    /// G12: retry an owner's close interrupted by BEFORE_ANY_PUSH. Success must publish the
    /// claim's end.
    #[test]
    fn g12_retried_close_publishes_the_end_of_the_claim() {
        let seen = crash_then_retry("close", &["--reason", "done"]);
        assert_eq!(seen, ended("closed"), "{}", unpublished("close", &seen));
    }

    /// G12: retry an owner's delete interrupted by BEFORE_ANY_PUSH. Success must publish the
    /// claim's end.
    #[test]
    fn g12_retried_delete_publishes_the_end_of_the_claim() {
        let seen = crash_then_retry("delete", &["--reason", "gone"]);
        assert_eq!(seen, ended("tombstone"), "{}", unpublished("delete", &seen));
    }

    /// G12: retry `update --status closed` interrupted by BEFORE_ANY_PUSH, with the same
    /// publication requirement as close.
    #[test]
    fn g12_retried_update_status_closed_publishes_the_end_of_the_claim() {
        let seen = crash_then_retry("update", &["--status", "closed"]);
        assert_eq!(
            seen,
            ended("closed"),
            "{}",
            unpublished("update --status closed", &seen)
        );
    }
}
