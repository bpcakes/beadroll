//! G30's fourth test (`docs/FINDINGS.md`) as a mixed-fleet case (`docs/RUST-PORT.md` §4.6): after
//! one machine tries to import a record 12,000 levels deep (refused: events nest at most 100 levels
//! and imported records 98), a peer whose reader stops lower keeps working and sees what the first
//! sees. Python's test ran the peer's bead under Python 3.12 or 3.13, whose json stops near 10,000
//! levels, against Python 3.14 on the importer; here the two machines run the two implementations,
//! whose reading ceilings differ (§5 row 7), each way round. Each machine names its binary, Python
//! 3.14's launcher or the Rust `bead`, whatever `BEAD_UNDER_TEST` and `BEAD_MIXED_WITH` say.

mod g30_deep_json_is_bounded_and_scoped {
    use std::collections::BTreeSet;
    use std::path::Path;

    use beadroll_test_support::{Json, Machine, code, parse_json};

    use crate::hardening::g30::g30_deep_json_is_bounded_and_scoped::{
        assert_no_crash, assert_works, record,
    };
    use crate::mixed::{python, rust};

    /// The ids of the issues a `bead list --json` printed: `{i["id"] for i in listed["issues"]}`.
    fn ids(listed: &Json) -> BTreeSet<String> {
        let issues = listed["issues"].as_array().expect("a list of issues");
        let id = |issue: &Json| issue["id"].as_str().expect("an id").to_owned();
        issues.iter().map(id).collect()
    }

    /// `test_peer_on_python_312_keeps_working_after_a_deep_import_elsewhere`, its machine a (the
    /// class's `g30`, which imports) running `importer` and its machine b (`g30b`, the peer)
    /// running `peer`.
    fn peer_keeps_working_after_a_deep_import_elsewhere(importer: &Path, peer: &Path) {
        let sandbox = crate::sandbox();
        // the class's `setUp`, on the importer's bead
        let a = Machine::new(&sandbox, "g30").with_bead(importer);
        a.bead(["init", "--prefix", "ex"]);
        a.bead(["create", "ordinary work"]);
        a.bead(["sync"]);
        let b = Machine::new(&sandbox, "g30b").with_bead(peer);
        let on = peer.display();
        assert_works(&b.run(["sync", "--json"]), &format!("{on}: first sync"));
        let path = sandbox.root().join("deep.jsonl");
        let deep = record("ex-deep", 12_000, "x_state");
        std::fs::write(&path, deep).expect("the record to import");
        let path = path.to_str().expect("a UTF-8 path");
        let imported = a.run(["import", path, "--json"]);
        // What matters is b afterwards, Python's test says, the import accepted or refused; the
        // port also holds the importer to what §4.6 says of it: refused whole, not a crash.
        let by = importer.display();
        assert_no_crash(&imported, &format!("{by}: the deep import"));
        let accepted = imported.returncode() == 0;
        assert!(
            !accepted,
            "{by} imported the deep record: {}",
            imported.stdout
        );
        assert_eq!(code(&imported), "VALIDATION", "{by}: the deep import");
        a.bead(["create", "made after the import"]);
        a.bead(["sync"]);
        let synced = b.run(["sync", "--json"]);
        assert_works(&synced, &format!("{on}: sync after a's import"));
        let listed = b.run(["list", "--all", "--limit", "0", "--json"]);
        assert_works(&listed, &format!("{on}: list"));
        let seen = parse_json(&listed.stdout).expect("bead list prints a record");
        let first = a.bead(["list", "--all", "--limit", "0"]);
        assert_eq!(ids(&seen), ids(&first.expect("bead list prints a record")));
    }

    /// Python imports, and its Rust peer keeps working and sees what it sees.
    #[test]
    fn peer_on_rust_keeps_working_after_a_deep_import_on_python() {
        peer_keeps_working_after_a_deep_import_elsewhere(&python(), &rust());
    }

    /// Rust imports, and its Python peer keeps working and sees what it sees.
    #[test]
    fn peer_on_python_keeps_working_after_a_deep_import_on_rust() {
        peer_keeps_working_after_a_deep_import_elsewhere(&rust(), &python());
    }
}
