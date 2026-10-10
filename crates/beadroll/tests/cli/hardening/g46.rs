//! G46: an import whose record depends on itself fails whole.

/// A record whose dependencies name its own id is imported as a self-edge: the issue is blocked by
/// itself and never ready, `dep cycles` reports it, while `dep add X X` refuses the same edge
/// (CYCLE) and br refuses the file ('Issue cannot depend on itself').
mod g46_import_accepts_self_dependency {
    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, code, json};

    /// `_g46_record`: the Beads record `key`, titled `key` in capitals, with a dependency on each
    /// `(target, type)` of `depends_on`.
    fn record(key: &str, depends_on: &[(&str, &str)]) -> Json {
        let title = key.to_uppercase();
        let mut record = json!({
            "id": key,
            "title": title,
            "status": "open",
            "priority": 2,
            "issue_type": "task",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "created_by": "seed",
        });
        if let (false, Json::Object(members)) = (depends_on.is_empty(), &mut record) {
            let dependencies = depends_on
                .iter()
                .map(|&(target, kind)| {
                    json!({"issue_id": key, "depends_on_id": target, "type": kind})
                })
                .collect();
            members.insert("dependencies".into(), Json::Array(dependencies));
        }
        record
    }

    /// `json.dumps(record)`: Python's default dialect, spaced and ASCII-escaped.
    fn dumps(record: &Json) -> String {
        write(record, Format::ASCII).expect("a record bead can read")
    }

    /// G46: an import whose record depends on itself fails whole with VALIDATION and writes
    /// nothing, whatever the dependency type, as `dep add X X` and br refuse it.
    #[test]
    fn import_refuses_a_self_dependency_whole() {
        let sandbox = crate::sandbox();
        let machine = Machine::new(&sandbox, "self");
        machine.bead(["init", "--prefix", "ex"]);
        for (n, kind) in ["blocks", "related", "parent-child"]
            .into_iter()
            .enumerate()
        {
            let path = sandbox.root().join(format!("self-{kind}.jsonl"));
            let (ok, own) = (format!("ex-ok{n}"), format!("ex-a{n}"));
            let text = dumps(&record(&ok, &[])) + "\n" + &dumps(&record(&own, &[(&own, kind)]));
            std::fs::write(&path, text + "\n").unwrap();
            let path = path.to_str().expect("a UTF-8 path");
            let proc = machine.run(["import", path, "--json"]);
            assert_ne!(
                proc.returncode(),
                0,
                "a {kind} self-dependency was imported: {}",
                proc.stdout
            );
            assert_eq!(code(&proc), "VALIDATION", "{kind}");
            let listed = machine.bead(["list", "--all", "--limit", "0"]);
            let listed = listed.expect("a listing");
            assert_eq!(
                listed.get("total"),
                Some(&Json::from(0)),
                "{kind}: a failed import wrote records"
            );
        }
        let cycles = machine.bead(["dep", "cycles"]).expect("a report of cycles");
        assert_eq!(
            cycles.get("count"),
            Some(&Json::from(0)),
            "imported self-dependencies form cycles"
        );
    }
}
