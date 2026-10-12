//! G54: cleaning up a stale twin must retain the accepted attempt's receipt. This `rust_only`
//! test injects `ownership.after-confirm=exit(97)` between confirmation and cleanup.

/// Link.merge's stale-twin rule (F11) calls drop_pending, which also deletes the receipt of the
/// attempt the remote accepted; the retry of that accepted command is then not recognised. Needs a
/// twin that differs from the accepted copy, which only local damage produces.
mod g54_stale_twin_rule_drops_the_receipt {
    use std::fs::OpenOptions;
    use std::io::Write as _;

    use beadroll_store::glob_ending;

    /// G54: stop release with ownership.after-confirm=exit(97), retaining its confirmed attempt,
    /// pending twin and receipt. Damage the twin and retry: release must be recognised and return
    /// success.
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
