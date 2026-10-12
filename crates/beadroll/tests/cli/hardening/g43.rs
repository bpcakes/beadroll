//! G43: a record renamed in a merge exports its dependencies under its own id.

/// When a record imported on two machines is renamed in the merge (ex-a -> ex-aX), the renamed
/// copy's still-unresolved imported dependency is exported with issue_id 'ex-a', the other
/// record's id; br refuses the whole file ('dependency issue_id ... does not match import issue
/// ...').
mod g43_renamed_record_exports_foreign_issue_id {
    use std::collections::BTreeSet;

    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, json, parse_json};

    /// G43: every exported dependency names its own record (issue_id == id), including a
    /// still-dangling imported dependency of a record that was renamed because two machines
    /// imported the same key.
    #[test]
    fn unresolved_dependency_is_exported_under_the_records_own_id() {
        let sandbox = crate::sandbox();
        let dep = json!({
            "issue_id": "ex-a",
            "depends_on_id": "external:other:ex-9",
            "type": "blocks",
            "created_at": "2026-01-01T00:00:00Z",
            "created_by": "seed",
            "metadata": "{}",
            "thread_id": "",
        });
        let rec = json!({
            "id": "ex-a",
            "title": "A",
            "status": "open",
            "priority": 2,
            "issue_type": "task",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "created_by": "seed",
            "dependencies": [dep],
        });
        let path = sandbox.root().join("ext.jsonl");
        let line = write(&rec, Format::ASCII).expect("a record bead can read") + "\n";
        std::fs::write(&path, line).unwrap();
        let path = path.to_str().expect("a UTF-8 path");
        let m1 = Machine::new(&sandbox, "m1");
        m1.bead(["init", "--prefix", "ex"]);
        m1.bead(["sync"]);
        let m2 = Machine::new(&sandbox, "m2");
        m2.bead(["sync"]);
        m1.bead(["import", path]);
        m2.bead(["import", path]);
        for machine in [&m1, &m2, &m1, &m2] {
            machine.bead(["sync"]);
        }
        // The records are ASCII and have no line separator other than newline.
        let records: Vec<Json> = m1
            .export()
            .lines()
            .map(|line| parse_json(line).unwrap_or_else(|error| panic!("{error}: {line}")))
            .collect();
        assert_eq!(records.len(), 2);
        let ids: BTreeSet<String> = records.iter().map(|r| member(r, "id")).collect();
        assert_eq!(ids.len(), 2);
        for exported in &records {
            let own = member(exported, "id");
            let dependencies = exported.get("dependencies").and_then(Json::as_array);
            let dependencies = dependencies.unwrap_or_else(|| panic!("{exported:?}"));
            let named: Vec<String> = dependencies.iter().map(|d| member(d, "issue_id")).collect();
            assert_eq!(
                named,
                std::slice::from_ref(&own),
                "record {own} exports a dependency whose issue_id names another record"
            );
        }
    }

    /// The text `record[key]`, which must be there.
    fn member(record: &Json, key: &str) -> String {
        let value = record.get(key).and_then(Json::as_str);
        value
            .unwrap_or_else(|| panic!("no {key} in {record:?}"))
            .to_owned()
    }
}
