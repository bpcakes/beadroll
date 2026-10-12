//! G45: `show` and `dep list` name an imported dependency on a key that is not an issue yet.

/// An imported dependency on a key that is not an issue yet appears only in export: show (JSON and
/// human) and dep list omit it, although `dep remove` accepts it and it blocks the issue as soon
/// as the key is imported.
mod g45_unresolved_imported_dependency_invisible {
    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, json};

    /// Spaced, ASCII-escaped JSON.
    fn dumps(value: &Json) -> String {
        write(value, Format::ASCII).expect("a value bead printed")
    }

    /// G45: show --json, human show, dep list --json and human dep list name an imported
    /// dependency on a key that is not an issue yet (once, although the record lists it twice).
    #[test]
    fn show_and_dep_list_report_an_unresolved_imported_dependency() {
        let sandbox = crate::sandbox();
        // The class's `setUp`: ex-a, depending twice on the key ex-b, the second time without a
        // type, imported on a new machine.
        let rec = json!({
            "id": "ex-a",
            "title": "A",
            "status": "open",
            "priority": 2,
            "issue_type": "task",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "created_by": "seed",
            "dependencies": [
                {"issue_id": "ex-a", "depends_on_id": "ex-b", "type": "blocks"},
                {"issue_id": "ex-a", "depends_on_id": "ex-b", "type": null}
            ],
        });
        let path = sandbox.root().join("a.jsonl");
        std::fs::write(&path, dumps(&rec) + "\n").unwrap();
        let machine = Machine::new(&sandbox, "vis");
        machine.bead(["init", "--prefix", "ex"]);
        machine.bead(["import", path.to_str().expect("a UTF-8 path")]);

        assert!(machine.export().contains("ex-b"));
        // ready now; it becomes blocked the moment ex-b is imported
        assert!(machine.ready().contains("ex-a"));
        let shown = dumps(&machine.show("ex-a"));
        assert!(
            shown.contains("ex-b"),
            "show --json hides the pending imported dependency: {shown}"
        );
        let listed = machine.bead(["dep", "list", "ex-a"]).unwrap_or(Json::Null);
        let listed = dumps(&listed);
        assert!(
            listed.contains("ex-b"),
            "dep list --json hides it: {listed}"
        );
        let human = machine.run(["show", "ex-a"]);
        assert_eq!(human.returncode(), 0, "{}", human.stderr);
        assert_eq!(
            human.stdout.matches("ex-b").count(),
            1,
            "human show hides it (or lists it twice): {}",
            human.stdout
        );
        let human_list = machine.run(["dep", "list", "ex-a"]);
        assert!(
            human_list.stdout.contains("ex-b"),
            "human dep list hides it: {}",
            human_list.stdout
        );
        let removed = machine.bead(["dep", "remove", "ex-a", "ex-b"]);
        let removed = removed.expect("dep remove prints the dependency");
        assert_eq!(removed.get("depends_on_id"), Some(&Json::from("ex-b")));
        assert!(
            !dumps(&machine.show("ex-a")).contains("ex-b"),
            "the retracted dependency is still shown"
        );
    }
}
