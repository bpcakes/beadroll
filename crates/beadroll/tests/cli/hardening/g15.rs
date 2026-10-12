//! G15: a key minted by `create --parent`, or one the merge renames a twin to, never takes a key
//! an imported dependency is still waiting for (`docs/FINDINGS.md`).

use std::path::Path;

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, json};

/// A Beads JSON record with `key`, `title`, `kind` and a dependency for each `(key, type)` in
/// `deps`.
fn record(key: &str, title: &str, deps: &[(&str, &str)], kind: &str) -> String {
    let dependencies: Vec<Json> = deps
        .iter()
        .map(|(d, t)| json!({"issue_id": key, "depends_on_id": (*d), "type": (*t)}))
        .collect();
    let record = json!({
        "id": key,
        "title": title,
        "status": "open",
        "priority": 2,
        "issue_type": kind,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed",
        "dependencies": dependencies,
    });
    write(&record, Format::ASCII).expect("a record bead can read")
}

/// `_g15_titles`: the titles of the dependencies `show` lists for `key` on `machine`.
fn titles(machine: &Machine, key: &str) -> Vec<String> {
    let shown = machine.show(key);
    let dependencies = shown.get("dependencies").and_then(Json::as_array);
    let dependencies = dependencies.unwrap_or_else(|| panic!("no dependencies in {shown:?}"));
    let title = |dependency: &Json| {
        let title = dependency.get("title").and_then(Json::as_str);
        let title = title.unwrap_or_else(|| panic!("no title in {dependency:?}"));
        title.to_owned()
    };
    dependencies.iter().map(title).collect()
}

/// The id of the issue a `--json` result is: `result["id"]`.
fn id(result: Option<Json>) -> String {
    let result = result.expect("bead printed a result");
    let id = result.get("id").and_then(Json::as_str);
    id.unwrap_or_else(|| panic!("no id in {result:?}"))
        .to_owned()
}

/// `path` as the text of a command-line argument.
fn utf8(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

/// mint_key (create --parent) and State.free_key (the rename of a key two machines minted
/// concurrently) avoid only keys that are issues, not keys an imported dependency is still waiting
/// for. A new or renamed issue can take such a key; the imported dependency then binds to that
/// unrelated issue, and the genuine record imported later is skipped as already known.
mod g15_new_key_captures_pending_imported_dependency {
    use std::collections::BTreeSet;

    use beadroll_test_support::{Json, Machine};

    use super::{id, record, titles, utf8};

    /// G15: `create --parent ex-p` does not mint ex-p.2 while an imported dependency waits for
    /// ex-p.2; the genuine ex-p.2 imported next is imported and is what ex-r depends on.
    #[test]
    fn local_child_key_does_not_capture_an_imported_dependency() {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "a");
        a.bead(["init", "--prefix", "ex"]);
        let first = sandbox.root().join("first.jsonl");
        let second = sandbox.root().join("second.jsonl");
        let lines = [
            record("ex-p", "parent", &[], "epic"),
            record(
                "ex-p.1",
                "imported child one",
                &[("ex-p", "parent-child")],
                "task",
            ),
            record("ex-r", "waits for ex-p.2", &[("ex-p.2", "blocks")], "task"),
        ];
        std::fs::write(&first, lines.join("\n") + "\n").unwrap();
        let genuine = record(
            "ex-p.2",
            "imported child two",
            &[("ex-p", "parent-child")],
            "task",
        );
        std::fs::write(&second, genuine + "\n").unwrap();
        a.bead(["import", utf8(&first)]);
        let local = id(a.bead(["create", "unrelated local child", "--parent", "ex-p"]));
        assert_ne!(
            local, "ex-p.2",
            "create --parent minted ex-p.2, the key an imported dependency waits for"
        );
        assert_eq!(titles(&a, "ex-r"), Vec::<String>::new());
        let imported = a
            .bead(["import", utf8(&second)])
            .expect("the import's result");
        let count = imported.get("imported");
        let count = count.unwrap_or_else(|| panic!("no imported count in {imported:?}"));
        assert_eq!(
            count,
            &Json::from(1),
            "the genuine ex-p.2 record was skipped"
        );
        assert_eq!(titles(&a, "ex-r"), ["imported child two"]);
        a.bead(["close", &local, "--reason", "done"]);
        let ready = a.ready();
        assert!(!ready.contains("ex-r"), "ex-r is in {ready:?}");
    }

    /// G15: when two machines both mint ex-p.1 and replay renames one of them, the rename skips
    /// ex-p.2, which an imported dependency (already applied in replay order) is waiting for.
    #[test]
    fn merge_rename_does_not_capture_an_imported_dependency() {
        let sandbox = crate::sandbox();
        let machines = ["m1", "m2", "m3"].map(|name| Machine::new(&sandbox, name));
        let [m1, m2, m3] = &machines;
        m1.bead(["init", "--prefix", "ex"]);
        let epic = sandbox.root().join("epic.jsonl");
        std::fs::write(&epic, record("ex-p", "parent epic", &[], "epic") + "\n").unwrap();
        m1.bead(["import", utf8(&epic)]);
        for machine in [m1, m2, m3] {
            machine.bead(["sync"]);
        }
        let waits = sandbox.root().join("waits.jsonl");
        let waiting = record("ex-r", "waits for ex-p.2", &[("ex-p.2", "blocks")], "task");
        std::fs::write(&waits, waiting + "\n").unwrap();
        // first by clock: applied before both children in replay order
        m3.bead(["import", utf8(&waits)]);
        assert_eq!(
            id(m1.bead(["create", "machine 1 child", "--parent", "ex-p"])),
            "ex-p.1"
        );
        assert_eq!(
            id(m2.bead(["create", "machine 2 child", "--parent", "ex-p"])),
            "ex-p.1"
        );
        for machine in [m3, m1, m2, m3, m1, m2] {
            machine.bead(["sync"]);
        }
        let exports: BTreeSet<String> = machines.iter().map(Machine::export).collect();
        assert_eq!(exports.len(), 1, "machines did not converge");
        for machine in &machines {
            let name = machine.name();
            assert_eq!(
                titles(machine, "ex-r"),
                Vec::<String>::new(),
                "{name}: the renamed child took ex-p.2 and captured ex-r's imported dependency"
            );
            let shown = machine.show("ex-r");
            let blocked = shown.get("blocked");
            let blocked = blocked.unwrap_or_else(|| panic!("no blocked in {shown:?}"));
            assert!(
                !blocked.truthy(),
                "{name}: ex-r is blocked by an unrelated child"
            );
        }
    }
}
