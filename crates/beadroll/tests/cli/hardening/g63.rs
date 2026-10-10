//! G63: a damaged line in a journal stays reported whatever else the journals hold.

mod g63_settle_keeps_line_level_integrity_reports {
    use std::path::{Path, PathBuf};

    use beadroll_test_support::{Json, Machine};

    /// The segments of a journal, `*.jsonl` in `dir`, by name: `sorted(dir.glob("*.jsonl"))`.
    fn segments(dir: &Path) -> Vec<PathBuf> {
        let entries = std::fs::read_dir(dir).expect("a journal's folder");
        let mut found: Vec<PathBuf> = entries
            .map(|entry| entry.expect("a directory entry").path())
            .filter(|path| {
                let name = path.file_name().expect("a name");
                name.as_encoded_bytes().ends_with(b".jsonl")
            })
            .collect();
        found.sort();
        found
    }

    /// The problem of each row `info` reports under integrity: `[i["problem"] for i in ...]`.
    fn problems(machine: &Machine) -> Vec<Json> {
        let info = machine.bead(["info"]).expect("bead info prints a record");
        let integrity = info.get("integrity").expect("info's integrity");
        let rows = integrity.as_array().expect("a list");
        rows.iter()
            .map(|row| row.get("problem").expect("a row's problem").clone())
            .collect()
    }

    /// G63: a "line is not valid JSON" report stays when a duplicate id (here a copy of one of the
    /// machine's own events) forces the journal to be rebuilt. `Journal.settle()` reset `invalid`
    /// to the unidentified records whenever it rebuilt, so the same damaged line was reported or
    /// not depending on what else the journals held: a duplicate id, a malformed event or a
    /// misplaced copy.
    #[test]
    fn unreadable_line_stays_reported_when_a_duplicate_forces_a_rebuild() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "a");
        a.bead(["init", "--prefix", "lr"]);
        a.bead(["create", "one"]);
        let info = a.bead(["info"]).expect("bead info prints a record");
        let own = info.get("machine").expect("info's machine");
        let own = own.as_str().expect("a machine id");
        let segment = segments(&a.store().join("events").join(own))
            .into_iter()
            .next()
            .expect("a segment of the machine's own journal");
        let text = std::fs::read_to_string(&segment).unwrap();
        // `splitlines()`: the events here are ASCII, with none of the other breaks Python splits at
        let first_event = text.lines().next().expect("an event");
        let foreign = a.store().join("events/fffffff0/000001.jsonl");
        std::fs::create_dir(foreign.parent().unwrap()).unwrap();
        std::fs::write(&foreign, "not json\n{}\n").unwrap();
        let not_json = Json::from("line is not valid JSON");
        let alone = problems(&a);
        // control: reported while nothing forces a rebuild
        assert!(alone.contains(&not_json), "{alone:?}");
        // a copy of one of a's own events
        std::fs::write(&foreign, format!("not json\n{first_event}\n")).unwrap();
        let with_copy = problems(&a);
        assert!(
            with_copy.contains(&not_json),
            "the damaged line is no longer reported once a copied event forces a rebuild: \
             {with_copy:?}"
        );
    }
}
