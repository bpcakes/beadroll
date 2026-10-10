//! V4: the retraction of an imported dependency, end to end. The replay half of V4, the
//! determinism of dangling histories under any delivery order, is beadroll-core's
//! `hardening::v4_dangling_replay_determinism`.

mod v4_dangling_orders_that_hold {
    //! End to end on two machines: the orders in which a retraction of an imported dependency
    //! already holds, and add-wins against a key-based retraction.

    use std::collections::BTreeSet;

    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, Sandbox, json};

    /// `_v4_record`: the Beads record `key`, titled in capitals, that depends on (is blocked by)
    /// each of `depends_on`.
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
            let dependencies = depends_on
                .iter()
                .map(|d| json!({"issue_id": key, "depends_on_id": (*d), "type": "blocks"}))
                .collect::<Vec<_>>();
            let Json::Object(members) = &mut record else {
                unreachable!("a record is an object")
            };
            members.insert("dependencies".into(), Json::Array(dependencies));
        }
        record
    }

    /// `write`: `records` as the JSONL file `<name>.jsonl` in the sandbox, each line as Python's
    /// `json.dumps` writes it; its path.
    fn write_records(sandbox: &Sandbox, name: &str, records: &[Json]) -> String {
        let path = sandbox.root().join(format!("{name}.jsonl"));
        let text: String = records
            .iter()
            .map(|r| write(r, Format::ASCII).expect("a record") + "\n")
            .collect();
        std::fs::write(&path, text).unwrap();
        path.to_str().expect("a UTF-8 path").to_owned()
    }

    /// `pair`: m1 imports ex-a with a dependency on ex-b, which exists nowhere yet (a dangling
    /// entry); m2 syncs it.
    fn pair(sandbox: &Sandbox) -> (Machine, Machine) {
        let m1 = Machine::new(sandbox, "m1");
        m1.bead(["init", "--prefix", "ex"]);
        m1.bead([
            "import",
            &write_records(sandbox, "a", &[record("ex-a", &["ex-b"])]),
        ]);
        m1.bead(["sync"]);
        let m2 = Machine::new(sandbox, "m2");
        m2.bead(["sync"]);
        assert_eq!(edges(&m2, "ex-a"), []);
        (m1, m2)
    }

    /// `settle`: every machine syncs, twice round, and then their exports are equal.
    fn settle(machines: &[&Machine]) {
        for machine in machines.iter().chain(machines) {
            machine.bead(["sync"]);
        }
        let exports: BTreeSet<String> = machines.iter().map(|m| m.export()).collect();
        assert_eq!(exports.len(), 1, "machines did not converge");
    }

    /// `edges`: `key`'s dependencies as `bead show` lists them, sorted (id, type) pairs.
    fn edges(machine: &Machine, key: &str) -> Vec<(String, String)> {
        let shown = machine.show(key);
        let text = |value: &Json| value.as_str().expect("a string").to_owned();
        let mut edges: Vec<(String, String)> =
            shown.get("dependencies").map_or_else(Vec::new, |deps| {
                let deps = deps.as_array().expect("dependencies are a list");
                deps.iter()
                    .map(|d| (text(&d["id"]), text(&d["dependency_type"])))
                    .collect()
            });
        edges.sort();
        edges
    }

    /// `unseen`: `machine` does not have `key` yet.
    fn unseen(machine: &Machine, key: &str) {
        let run = machine.run(["show", key, "--json"]);
        assert_ne!(run.returncode(), 0, "{} already has {key}", machine.name());
    }

    /// One edge, `[(id, "blocks")]`, as `edges` lists it.
    fn blocks(id: &str) -> Vec<(String, String)> {
        vec![(id.to_owned(), "blocks".to_owned())]
    }

    /// Verified (V4): a key-based `dep remove` whose event sorts before the target's create holds:
    /// m1 removes ex-a's dangling dependency on ex-b before m2 imports ex-b (which depends on
    /// ex-a); after settling, ex-a has no edge and there is no cycle. The review ran this end to
    /// end: 3/3 order tests pass on the current bead and on the full patch.
    #[test]
    fn key_retraction_that_sorts_first_holds() {
        let sandbox = crate::sandbox();
        let (m1, m2) = pair(&sandbox);
        unseen(&m1, "ex-b");
        m1.bead(["dep", "remove", "ex-a", "ex-b"]);
        m2.bead([
            "import",
            &write_records(&sandbox, "b", &[record("ex-b", &["ex-a"])]),
        ]);
        settle(&[&m1, &m2]);
        assert_eq!(edges(&m1, "ex-a"), []);
        assert_eq!(edges(&m2, "ex-a"), []);
        let cycles = m2.bead(["dep", "cycles"]).expect("dep cycles");
        assert_eq!(cycles["count"].as_i64(), Some(0), "{cycles:?}");
    }

    /// Verified (V4): once the target's create has arrived and the dangling entry has become an
    /// edge, a uid-based `dep remove` takes it away for good on every machine. The review ran this
    /// end to end: 3/3 order tests pass on the current bead and on the full patch.
    #[test]
    fn uid_retraction_after_the_target_arrived_holds() {
        let sandbox = crate::sandbox();
        let (m1, m2) = pair(&sandbox);
        m2.bead([
            "import",
            &write_records(&sandbox, "b", &[record("ex-b", &["ex-a"])]),
        ]);
        m2.bead(["sync"]);
        m1.bead(["sync"]);
        assert_eq!(edges(&m1, "ex-a"), blocks("ex-b"));
        m1.bead(["dep", "remove", "ex-a", "ex-b"]);
        settle(&[&m1, &m2]);
        assert_eq!(edges(&m1, "ex-a"), []);
        assert_eq!(edges(&m2, "ex-a"), []);
    }

    /// Verified (V4): add-wins: m2 sees ex-b arrive, removes the imported edge and adds it back
    /// explicitly, while m1 (which has not seen ex-b) concurrently removes the dependency by key;
    /// after settling, the explicit edge ex-a -> ex-b (blocks) remains on both machines. The review
    /// ran this end to end: 3/3 order tests pass on the current bead and on the full patch.
    #[test]
    fn concurrent_explicit_add_survives_a_key_retraction() {
        let sandbox = crate::sandbox();
        let (m1, m2) = pair(&sandbox);
        m2.bead([
            "import",
            &write_records(&sandbox, "b", &[record("ex-b", &[])]),
        ]);
        m2.bead(["dep", "remove", "ex-a", "ex-b"]);
        m2.bead(["dep", "add", "ex-a", "ex-b"]);
        unseen(&m1, "ex-b");
        // key-based, concurrent with m2's explicit add
        m1.bead(["dep", "remove", "ex-a", "ex-b"]);
        settle(&[&m1, &m2]);
        assert_eq!(edges(&m1, "ex-a"), blocks("ex-b"));
        assert_eq!(edges(&m2, "ex-a"), blocks("ex-b"));
    }
}
