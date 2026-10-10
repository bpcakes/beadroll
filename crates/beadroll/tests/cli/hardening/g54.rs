//! G54 (`docs/FINDINGS.md`): the stale-twin rule also deleted the accepted attempt's receipt. The
//! Python test stops a release in process between confirming its accepted attempt and cleaning up,
//! by patching `drop_pending` to raise; here the `ownership.after-confirm=exit(97)` failpoint stops
//! it at the same moment, which only the Rust binary can: a `rust_only` test.

/// Link.merge's stale-twin rule (F11) calls drop_pending, which also deletes the receipt of the
/// attempt the remote accepted; the retry of that accepted command is then not recognised. Needs a
/// twin that differs from the accepted copy, which only local damage produces.
mod g54_stale_twin_rule_drops_the_receipt {
    use std::fs::OpenOptions;
    use std::io::Write as _;

    use beadroll_store::glob_ending;

    /// G54: after a crash between confirming an accepted release and cleaning up, with the pending
    /// twin damaged, the retried `release X` is recognised (exit 0), not NOT_OWNER.
    ///
    /// Replaces `G54_StaleTwinRuleDropsTheReceipt`'s
    /// `test_damaged_twin_of_an_accepted_release_keeps_its_receipt`: where its patched
    /// `drop_pending` raises "crash after local confirm", the failpoint ends the release with exit
    /// status 97, its attempt confirmed and its pending twin and receipt in place.
    #[test]
    fn damaged_twin_of_an_accepted_release_keeps_its_receipt() {
        let sandbox = crate::sandbox();
        let a = crate::rust_only(&sandbox, "g54-twin");
        a.bead(["init", "--prefix", "ex"]);
        let created = a.bead(["create", "X"]).expect("the new issue");
        let x = created["id"].as_str().expect("an id").to_owned();
        a.bead(["claim", &x]);
        let crashed = a
            .cmd(["release", &x])
            .env("BEAD_FAILPOINTS", "ownership.after-confirm=exit(97)")
            .run();
        assert_eq!(
            crashed.returncode(),
            97,
            "the release did not stop after its local confirmation: {}",
            crashed.stderr
        );
        let pending = a.store().join("pending");
        assert_eq!(glob_ending(&pending, ".command").len(), 1);
        let twin = glob_ending(&pending, ".json");
        let twin = twin.first().expect("the pending twin");
        // local damage: the twin no longer matches the accepted copy
        let mut file = OpenOptions::new().append(true).open(twin).unwrap();
        file.write_all(b"\n").unwrap();
        drop(file);
        let retry = a.run(["release", &x, "--json"]);
        assert_eq!(
            retry.returncode(),
            0,
            "retry of the accepted release not recognised: {}",
            retry.stderr
        );
        let shown = a.show(&x);
        assert!(shown.get("claim").is_none(), "{shown:?}");
    }
}
