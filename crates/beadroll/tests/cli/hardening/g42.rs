//! G42: taking back an imported dependency, or moving an issue away from an imported parent,
//! survives a merge that binds the dependency's key to another machine's copy of that key.

/// An imported dependency on a key that did not exist yet binds, after a merge, to whichever
/// concurrent copy of that key sorts first. A writer who took the dependency back named the copy
/// it saw (dep_remove by uid); when the other copy keeps the key, the retraction misses it and the
/// dependency is live again on every machine.
mod g42_uid_retraction_misses_twin_of_imported_key {
    use std::collections::BTreeSet;

    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, Sandbox, json};

    /// `_g42_record`: the Beads record `key` titled `title`, depending on `depends_on`, when there
    /// is one, by a dependency of type `kind`.
    fn record(key: &str, title: &str, depends_on: Option<&str>, kind: &str) -> Json {
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
        if let (Some(depends_on), Json::Object(members)) = (depends_on, &mut record) {
            let dependency = json!({"issue_id": key, "depends_on_id": depends_on, "type": kind});
            members.insert("dependencies".into(), Json::Array(vec![dependency]));
        }
        record
    }

    /// One record as spaced, ASCII-escaped JSON followed by a newline.
    fn line(record: &Json) -> String {
        write(record, Format::ASCII).expect("a record bead can read") + "\n"
    }

    /// The id of the issue a `--json` result is: `result["id"]`.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        result["id"].as_str().expect("an id").to_owned()
    }

    /// `issue["dependencies"]`: the dependencies `show` lists for an issue.
    fn listed(issue: &Json) -> &[Json] {
        let listed = issue.get("dependencies").and_then(Json::as_array);
        listed.unwrap_or_else(|| panic!("no dependencies in {issue:?}"))
    }

    /// The text `dependency[key]`, which must be there.
    fn text(dependency: &Json, key: &str) -> String {
        let value = dependency.get(key).and_then(Json::as_str);
        value
            .unwrap_or_else(|| panic!("no {key} in {dependency:?}"))
            .to_owned()
    }

    /// `[(d["id"], d["dependency_type"]) for d in issue["dependencies"]]`.
    fn dependencies(issue: &Json) -> Vec<(String, String)> {
        let pair = |d: &Json| (text(d, "id"), text(d, "dependency_type"));
        listed(issue).iter().map(pair).collect()
    }

    /// `[d["id"] for d in issue["dependencies"]]`.
    fn dependency_ids(issue: &Json) -> Vec<String> {
        listed(issue).iter().map(|d| text(d, "id")).collect()
    }

    /// `_g42_machines`: m1 imports ex-a, which depends on the key ex-b (not an issue yet) by a
    /// dependency of type `kind`, creates Q and publishes; m2 and m3 sync, then both import ex-b,
    /// m3 first, so that m3's copy keeps the key after the merge and m2's is renamed. Machines m1,
    /// m2 and m3, and Q's id.
    fn machines(sandbox: &Sandbox, kind: &str) -> (Machine, Machine, Machine, String) {
        let (a, b) = (
            sandbox.root().join("a.jsonl"),
            sandbox.root().join("b.jsonl"),
        );
        std::fs::write(&a, line(&record("ex-a", "A", Some("ex-b"), kind))).unwrap();
        std::fs::write(&b, line(&record("ex-b", "B", None, "blocks"))).unwrap();
        let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());
        let m1 = Machine::new(sandbox, "m1");
        m1.bead(["init", "--prefix", "ex"]);
        m1.bead(["import", a]);
        let q = id(m1.bead(["create", "Q"]));
        m1.bead(["sync"]);
        let (m2, m3) = (Machine::new(sandbox, "m2"), Machine::new(sandbox, "m3"));
        m2.bead(["sync"]);
        m3.bead(["sync"]);
        m3.bead(["import", b]); // earlier id: this copy keeps ex-b after the merge
        m2.bead(["import", b]); // renamed after the merge
        assert_eq!(
            dependencies(&m2.show("ex-a")),
            [("ex-b".to_owned(), kind.to_owned())]
        );
        (m1, m2, m3, q)
    }

    /// `_g42_settle`: every machine syncs twice, and then they export the same records.
    fn settle(machines: &[&Machine]) {
        for machine in machines.iter().chain(machines) {
            machine.bead(["sync"]);
        }
        let exports: BTreeSet<String> = machines.iter().map(|m| m.export()).collect();
        assert_eq!(exports.len(), 1, "machines did not converge");
    }

    /// G42: `dep remove ex-a ex-b` of an imported dependency stays removed after sync even when a
    /// concurrent import of ex-b on another machine sorts first and keeps the key.
    #[test]
    fn dep_remove_of_an_imported_dependency_survives_the_merge() {
        let sandbox = crate::sandbox();
        let (m1, m2, m3, _) = machines(&sandbox, "blocks");
        m2.bead(["dep", "remove", "ex-a", "ex-b"]); // by uid: m2's own copy of ex-b
        let none = Json::Array(Vec::new());
        assert_eq!(m2.show("ex-a").get("dependencies"), Some(&none));
        settle(&[&m1, &m2, &m3]);
        let listed = m2
            .bead(["list", "--all", "--limit", "0"])
            .expect("a listing");
        let issues = listed["issues"].as_array().expect("the listing's issues");
        assert_eq!(issues.len(), 4); // ex-a, Q, ex-b, renamed twin
        assert_eq!(
            m2.show("ex-a").get("dependencies"),
            Some(&none),
            "the imported dependency m2 took back is live again, bound to the other copy of ex-b"
        );
        assert!(m2.ready().contains("ex-a"));
    }

    /// G42: `update --parent Q` that moves ex-a away from an imported parent-child dependency
    /// keeps it moved after sync even when a concurrent import of the old parent's key sorts
    /// first.
    #[test]
    fn reparenting_away_from_an_imported_parent_survives_the_merge() {
        let sandbox = crate::sandbox();
        let (m1, m2, m3, q) = machines(&sandbox, "parent-child");
        m2.bead(["update", "ex-a", "--parent", &q]); // removes the imported parent edge by uid
        assert_eq!(dependency_ids(&m2.show("ex-a")), std::slice::from_ref(&q));
        settle(&[&m1, &m2, &m3]);
        assert_eq!(
            dependency_ids(&m2.show("ex-a")),
            [q],
            "the imported parent m2 moved ex-a away from is a parent again after the merge"
        );
    }
}
