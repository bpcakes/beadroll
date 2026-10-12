//! G44: a dependency whose target arrives later, by another import or from another machine, exports
//! with a string `created_at` and `created_by`, as one imported with its target does.

/// A dependency without created_at/created_by: imported in one batch with its target, the dep_add
/// stamps it; when the target arrives later (another import or another machine),
/// materialise_dangling copies only the keys present and export writes "created_at": null,
/// "created_by": null, which br refuses ('invalid type: null, expected an RFC 3339 formatted date
/// and time string').
mod g44_materialised_dependency_exports_null_meta {
    use std::path::PathBuf;

    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, Sandbox, json, parse_json};

    /// `_g44_record`: the Beads record `key`, titled `key` in capitals, depending on each of
    /// `depends_on` by a `blocks` dependency that has no `created_at` or `created_by`, as in the
    /// shipped incremental-import test.
    fn record(key: &str, depends_on: &[&str]) -> Json {
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
                .map(|&target| json!({"issue_id": key, "depends_on_id": target, "type": "blocks"}))
                .collect();
            members.insert("dependencies".into(), Json::Array(dependencies));
        }
        record
    }

    /// The file `NAME.jsonl` in the sandbox's root.
    fn path(sandbox: &Sandbox, name: &str) -> PathBuf {
        sandbox.root().join(format!("{name}.jsonl"))
    }

    /// Fixtures: a.jsonl holds ex-a depending on ex-b; b.jsonl holds ex-b; ab.jsonl holds both.
    /// One spaced, ASCII-escaped JSON record per line.
    fn setup(sandbox: &Sandbox) {
        let (a, b) = (record("ex-a", &["ex-b"]), record("ex-b", &[]));
        for (name, records) in [("a", vec![&a]), ("b", vec![&b]), ("ab", vec![&a, &b])] {
            let text: String = records
                .into_iter()
                .map(|r| write(r, Format::ASCII).expect("a record bead can read") + "\n")
                .collect();
            std::fs::write(path(sandbox, name), text).unwrap();
        }
    }

    /// `str(self.box.root / "NAME.jsonl")`: the path of one of the files `setup` wrote.
    fn file(sandbox: &Sandbox, name: &str) -> String {
        let path = path(sandbox, name);
        path.to_str().expect("a UTF-8 path").to_owned()
    }

    /// `_g44_dependency`: the first dependency of the record `key` in `machine`'s export.
    fn dependency(machine: &Machine, key: &str) -> Json {
        // The records are ASCII and have no line separator other than newline.
        let export = machine.export();
        let record = export
            .lines()
            .map(|line| parse_json(line).unwrap_or_else(|error| panic!("{error}: {line}")))
            .find(|r| r.get("id").expect("an exported id").as_str() == Some(key));
        let record = record.unwrap_or_else(|| panic!("no {key} in the export: {export}"));
        let dependencies = record.get("dependencies").and_then(Json::as_array);
        let first = dependencies.and_then(|d| d.first());
        first
            .unwrap_or_else(|| panic!("{key} exports no dependency: {record:?}"))
            .clone()
    }

    /// Whether `dependency[key]` is a string, as `assertIsInstance(dependency[key], str)` checks;
    /// a member that is not there fails the test.
    fn is_string(dependency: &Json, key: &str) -> bool {
        let value = dependency.get(key);
        let value = value.unwrap_or_else(|| panic!("no {key} in {dependency:?}"));
        value.as_str().is_some()
    }

    /// G44: importing the dependent and its target in two batches exports the dependency with a
    /// string created_at and created_by, as a one-shot import of the same records does.
    #[test]
    fn incremental_import_stamps_the_dependency_like_one_shot() {
        let sandbox = crate::sandbox();
        setup(&sandbox);
        let split = Machine::new(&sandbox, "split");
        split.bead(["init", "--prefix", "ex"]);
        split.bead(["import", &file(&sandbox, "a")]);
        split.bead(["import", &file(&sandbox, "b")]);
        let one = Machine::new(&sandbox, "one");
        one.bead(["init", "--prefix", "ex"]);
        one.bead(["import", &file(&sandbox, "ab")]);
        assert!(is_string(&dependency(&one, "ex-a"), "created_at"));
        assert!(is_string(&dependency(&one, "ex-a"), "created_by"));
        let dep = dependency(&split, "ex-a");
        assert!(
            is_string(&dep, "created_at"),
            "incremental import exported a null created_at: {dep:?}"
        );
        assert!(
            is_string(&dep, "created_by"),
            "incremental import exported a null created_by: {dep:?}"
        );
    }

    /// G44: when the dependent and its target are imported on two machines and meet by sync, both
    /// machines export the dependency with a string created_at and created_by.
    #[test]
    fn target_imported_on_another_machine_stamps_the_dependency() {
        let sandbox = crate::sandbox();
        setup(&sandbox);
        let (x, y) = (Machine::new(&sandbox, "x"), Machine::new(&sandbox, "y"));
        x.bead(["init", "--prefix", "ex"]);
        x.bead(["sync"]);
        y.bead(["sync"]);
        x.bead(["import", &file(&sandbox, "a")]);
        y.bead(["import", &file(&sandbox, "b")]);
        for machine in [&x, &y, &x, &y] {
            machine.bead(["sync"]);
        }
        assert_eq!(x.export(), y.export());
        let dep = dependency(&x, "ex-a");
        assert_eq!(dep.get("depends_on_id"), Some(&Json::from("ex-b")));
        assert!(
            is_string(&dep, "created_at"),
            "the synced dependency exported a null created_at: {dep:?}"
        );
        assert!(
            is_string(&dep, "created_by"),
            "the synced dependency exported a null created_by: {dep:?}"
        );
    }
}
