//! G14: retracting an imported dependency by key survives a concurrent create with an earlier id.
//! The clock does not decide the result. The replay-only test is in beadroll-core's
//! `tests/core/hardening/g14_key_retraction_lost_to_concurrent_target.rs`.

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, Sandbox, json};

/// `_g14_record`: the Beads record of `key`, titled `key` in capitals, with a `blocks` dependency on
/// each key of `depends_on`, if there is any.
fn record(key: &str, depends_on: &[&str]) -> Json {
    let mut record = json!({
        "id": key,
        "title": (key.to_uppercase()),
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed",
    });
    if !depends_on.is_empty() {
        let dependencies: Vec<Json> = depends_on
            .iter()
            .map(|d| json!({"issue_id": key, "depends_on_id": (*d), "type": "blocks"}))
            .collect();
        match &mut record {
            Json::Object(members) => members.insert("dependencies".into(), dependencies.into()),
            other => panic!("{other:?} is not an object"),
        };
    }
    record
}

/// Writes `records` as spaced, ASCII-escaped JSON lines to `NAME.jsonl` in the sandbox and
/// returns its path.
fn jsonl_file(sandbox: &Sandbox, name: &str, records: &[Json]) -> String {
    let path = sandbox.root().join(format!("{name}.jsonl"));
    let text: String = records
        .iter()
        .map(|record| write(record, Format::ASCII).expect("a record bead can read") + "\n")
        .collect();
    std::fs::write(&path, text).unwrap();
    path.to_str().expect("a UTF-8 path").to_owned()
}

/// `_g14_edges`: the dependencies `show` lists for `key` on `machine`, as sorted pairs of the id
/// and the dependency's type; none when it lists no dependencies.
fn edges(machine: &Machine, key: &str) -> Vec<(String, String)> {
    let shown = machine.show(key);
    let Some(dependencies) = shown.get("dependencies") else {
        return Vec::new();
    };
    let dependencies = dependencies
        .as_array()
        .unwrap_or_else(|| panic!("dependencies that are not a list: {shown:?}"));
    let member = |dependency: &Json, name: &str| {
        let value = dependency.get(name).and_then(Json::as_str);
        let value = value.unwrap_or_else(|| panic!("no {name} in {dependency:?}"));
        value.to_owned()
    };
    let mut edges: Vec<(String, String)> = dependencies
        .iter()
        .map(|d| (member(d, "id"), member(d, "dependency_type")))
        .collect();
    edges.sort();
    edges
}

/// A key-based dep_remove (the target is not an issue on the remover's machine) only filters
/// `dangling`. When another machine's concurrent import of the target has the earlier event id,
/// replay has already turned the entry into an edge, so the retraction is lost and the edge comes
/// back.
mod g14_key_retraction_lost_to_concurrent_target {
    use std::collections::BTreeSet;

    use beadroll_test_support::{Json, Machine, Sandbox};

    use super::{edges, jsonl_file, record};

    /// `_g14_pair`: machine m1 has initialised tracker `ex`, imported ex-a, which depends on ex-b,
    /// an issue nowhere yet, and synced; then machine m2 has synced.
    fn pair(sandbox: &Sandbox) -> (Machine, Machine) {
        let m1 = Machine::new(sandbox, "m1");
        m1.bead(["init", "--prefix", "ex"]);
        m1.bead([
            "import",
            &jsonl_file(sandbox, "a", &[record("ex-a", &["ex-b"])]),
        ]);
        m1.bead(["sync"]);
        let m2 = Machine::new(sandbox, "m2");
        m2.bead(["sync"]);
        (m1, m2)
    }

    /// `_g14_settle`: each machine syncs, in turn, twice; then they must export the same records.
    fn settle(machines: &[&Machine]) {
        for machine in machines.iter().chain(machines) {
            machine.bead(["sync"]);
        }
        let exports: BTreeSet<String> = machines.iter().map(|m| m.export()).collect();
        assert_eq!(exports.len(), 1, "machines did not converge");
    }

    /// G14: `dep remove ex-a ex-b` of an imported dependency on a key that is not an issue yet
    /// stays removed after sync even when another machine's concurrent import of ex-b has the
    /// earlier id.
    #[test]
    fn dep_remove_of_a_dangling_key_survives_a_concurrent_target_import() {
        let sandbox = crate::sandbox();
        let (m1, m2) = pair(&sandbox);
        let none: Vec<(String, String)> = Vec::new();
        // earlier id: sorts first
        m2.bead([
            "import",
            &jsonl_file(&sandbox, "b", &[record("ex-b", &["ex-a"])]),
        ]);
        // m1 has not seen ex-b: the removal is key-based
        m1.bead(["dep", "remove", "ex-a", "ex-b"]);
        assert_eq!(edges(&m1, "ex-a"), none);
        settle(&[&m1, &m2]);
        assert_eq!(
            edges(&m1, "ex-a"),
            none,
            "the dependency m1 took back came back once the concurrent import of ex-b was merged"
        );
        let ready = m1.ready();
        assert!(ready.contains("ex-a"), "ex-a is not in {ready:?}");
        let cycles = m1.bead(["dep", "cycles"]).expect("the cycles");
        let count = cycles
            .get("count")
            .unwrap_or_else(|| panic!("no count in {cycles:?}"));
        assert_eq!(count, &Json::from(0));
    }

    /// G14: a re-import that no longer lists the dangling dependency (a key-based retraction)
    /// stays removed after sync even when a concurrent import of the target sorts first.
    #[test]
    fn reimport_that_drops_the_dependency_survives_a_concurrent_target_import() {
        let sandbox = crate::sandbox();
        let (m1, m2) = pair(&sandbox);
        m2.bead(["import", &jsonl_file(&sandbox, "b", &[record("ex-b", &[])])]);
        m1.bead([
            "import",
            &jsonl_file(&sandbox, "cleared", &[record("ex-a", &[])]),
        ]);
        settle(&[&m1, &m2]);
        assert_eq!(
            edges(&m2, "ex-a"),
            Vec::<(String, String)>::new(),
            "the re-import's retraction was undone by the concurrent import of its target"
        );
    }
}
