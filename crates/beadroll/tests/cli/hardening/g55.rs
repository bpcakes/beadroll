//! G55: command settlement assumed every valid JSON receipt was an object; a
//! receipt holding `[]` made every coordinated command of every actor using that store crash, and
//! sync never swept it (`load_pending` sweeps `*.json` only).

mod g55_receipt_that_is_not_an_object_blocks_every_claim {
    use beadroll_test_support::{Json, Machine};

    /// The id of the issue a `--json` result is: `result["id"]`.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        result["id"].as_str().expect("an id").to_owned()
    }

    /// G55: a `pending/*.command` receipt holding valid JSON that is not an object is dropped like
    /// an unreadable one: `claim X` of any actor goes through instead of crashing, and the receipt
    /// is gone afterwards.
    #[test]
    fn list_receipt_does_not_block_claims() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "g55-shape");
        a.bead(["init", "--prefix", "ex"]);
        let x = id(a.bead(["create", "X"]));
        let y = id(a.bead(["create", "Y"]));
        a.bead(["sync"]);
        let pending = a.store().join("pending");
        if !pending.is_dir() {
            std::fs::create_dir(&pending).unwrap();
        }
        let receipt = pending.join("01900000-0000-7000-8000-0000000000bb.command");
        std::fs::write(&receipt, "[]").unwrap();
        assert_eq!(a.run(["sync"]).returncode(), 0);
        assert!(receipt.exists()); // sync does not sweep it
        let claim = a
            .cmd(["claim", &x, "--json"])
            .thread("g55-other-thread")
            .run();
        assert!(!claim.crashed(), "{}", claim.stderr);
        assert_eq!(claim.returncode(), 0, "{}", claim.stderr);
        let mine = a.run(["claim", &y, "--json"]);
        assert_eq!(mine.returncode(), 0, "{}", mine.stderr);
        assert!(
            !receipt.exists(),
            "the receipt that is not an object was kept"
        );
    }
}
