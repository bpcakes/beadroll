//! `tests/test_replay.py`'s two command-line tests, ported: a claim belongs to its thread's whole
//! id, not to the eight characters bead shows of it, and a parent cycle that two machines' edits
//! make once they merge is reported, with both issues ready, as `br` 0.5.7 has it. The module's
//! other tests check replay in-process and are beadroll-core's
//! (`crates/beadroll-core/tests/core/replay.rs`).

mod replay_audit_thread_ownership {
    use beadroll_test_support::{Machine, code};

    /// From the first independent audit: a thread whose id begins with the same eight characters
    /// as the owner's is another thread, which cannot release the owner's claim.
    #[test]
    fn distinct_threads_with_shared_first_eight_characters_are_not_same_owner() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "ownership");
        a.bead(["init", "--prefix", "au"]);
        let created = a.bead(["create", "thread ownership"]).expect("the issue");
        let issue = created["id"].as_str().expect("the issue's id").to_owned();
        let claim = a.cmd(["claim", &issue]);
        claim.thread("01a0c450-0001-7000-8000-000000000001").json();
        let release = a.cmd(["release", &issue, "--json"]);
        let proc = release.thread("01a0c450-0002-7000-8000-000000000002").run();
        assert_ne!(
            proc.returncode(),
            0,
            "different thread sharing displayed 8-char prefix released the claim"
        );
        assert_eq!(code(&proc), "NOT_OWNER");
    }
}

mod replay_reaudit {
    use std::collections::BTreeSet;

    use beadroll_test_support::{Json, Machine, json};

    /// The id of the issue `bead create` printed.
    fn id(created: Option<Json>) -> String {
        let created = created.expect("the new issue");
        created["id"].as_str().expect("the issue's id").to_owned()
    }

    /// From the second independent audit: a makes y the parent of x while b makes x the parent of
    /// y; once both edits have merged, `dep cycles` finds the one cycle, and both issues are ready,
    /// as `br` 0.5.7 has it for issues that are not epics. The class's `setUp` points the
    /// in-process package at a scratch home, which the machines never see: their environment drops
    /// every `BEAD_*` variable.
    #[test]
    fn merged_parent_cycle_matches_br_readiness() {
        let sandbox = crate::sandbox();
        let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
        a.bead(["init", "--prefix", "ex"]);
        let x = id(a.bead(["create", "first task"]));
        let y = id(a.bead(["create", "second task"]));
        a.bead(["sync"]);
        b.bead(["sync"]);
        a.bead(["update", &x, "--parent", &y]);
        b.bead(["update", &y, "--parent", &x]);
        for m in [&a, &b, &a, &b] {
            m.bead(["sync"]);
        }
        let cycles = a
            .bead(["dep", "cycles"])
            .expect("bead dep cycles prints a record");
        assert_eq!(cycles["count"], json!(1));
        assert_eq!(
            a.ready(),
            BTreeSet::from([x, y]),
            "non-epic parent cycles match br 0.5.7 readiness"
        );
    }
}
