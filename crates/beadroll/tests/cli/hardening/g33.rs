//! G33: two records with one id in one import fail the import whole, as br refuses them; the same
//! export twice in one file never gives two copies of an issue nor unblocks one.

use beadroll_core::json::Object;
use beadroll_test_support::{Json, json};

/// `_g33_common`: a Beads record's required fields.
fn common() -> Json {
    json!({
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed",
    })
}

/// The Beads record `{"id": KEY, "title": TITLE, **_g33_common()}`, then the members of `more`.
fn record(key: &str, title: &str, more: Json) -> Json {
    let mut members = Object::new();
    members.insert("id".into(), Json::from(key));
    members.insert("title".into(), Json::from(title));
    for part in [common(), more] {
        match part {
            Json::Object(part) => members.extend(part),
            other => panic!("{other:?} is not an object"),
        }
    }
    Json::Object(members)
}

/// `_g33_tail`: the last line of `text` that is not blank, or nothing.
fn tail(text: &str) -> &str {
    let mut lines = text.trim().split('\n');
    lines.rfind(|line| !line.trim().is_empty()).unwrap_or("")
}

mod g33_duplicate_ids_in_one_import_fail_whole {
    use std::path::PathBuf;

    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, Sandbox, code, json};

    use super::{record, tail};

    /// The class's `setUp`: machine g33, its tracker initialised with the prefix `ex`.
    fn setup(sandbox: &Sandbox) -> Machine {
        let m = Machine::new(sandbox, "g33");
        m.bead(["init", "--prefix", "ex"]);
        m
    }

    /// `write`: `records` in the sandbox's file `name`, each line as `json.dumps(record) + "\n"`.
    fn write_records(sandbox: &Sandbox, name: &str, records: &[Json]) -> PathBuf {
        let path = sandbox.root().join(name);
        let lines: String = records
            .iter()
            .map(|record| write(record, Format::ASCII).unwrap() + "\n")
            .collect();
        std::fs::write(&path, lines).unwrap();
        path
    }

    /// The ids of every issue, sorted: `sorted(i["id"] for i in bead list --all --limit 0)`.
    fn sorted_ids(m: &Machine) -> Vec<String> {
        let listed = m.bead(["list", "--all", "--limit", "0"]).unwrap();
        let issues = listed["issues"].as_array().expect("the listed issues");
        let mut ids: Vec<String> = issues
            .iter()
            .map(|issue| issue["id"].as_str().expect("an id").to_owned())
            .collect();
        ids.sort();
        ids
    }

    /// G33: two records with one id in an import are minted as two issues (the second renamed, and
    /// the first record's blockers moved onto it) instead of failing whole with VALIDATION, as br
    /// does.
    #[test]
    fn duplicate_ids_in_one_import_fail_whole() {
        let sandbox = crate::sandbox();
        let m = setup(&sandbox);
        let records = [
            record("ex-a", "first", json!({})),
            record("ex-a", "second", json!({})),
        ];
        let path = write_records(&sandbox, "dup.jsonl", &records);
        let proc = m.run(["import", path.to_str().unwrap(), "--json"]);
        assert!(!proc.crashed(), "import crashed: {}", tail(&proc.stderr));
        assert_ne!(
            proc.returncode(),
            0,
            "import with a repeated id was accepted: {}",
            proc.stdout.trim()
        );
        assert_eq!(code(&proc), "VALIDATION");
        let listed = m.bead(["list", "--all", "--limit", "0"]).unwrap();
        assert_eq!(listed["total"].as_i64(), Some(0), "{listed:?}");
    }

    /// G33: the same export twice in one file (a union merge of issues.jsonl) gives two claimable
    /// copies of each issue and leaves a blocked issue ready; it must be refused (or mean the same
    /// as once).
    #[test]
    fn concatenated_export_does_not_double_issues_or_unblock_them() {
        let sandbox = crate::sandbox();
        let m = setup(&sandbox);
        let blocked = json!({"dependencies": [{"depends_on_id": "ex-x", "type": "blocks"}]});
        let one = [record("ex-x", "x", json!({})), record("ex-y", "y", blocked)];
        let twice = [one.clone(), one.clone()].concat();
        let path = write_records(&sandbox, "twice.jsonl", &twice);
        let proc = m.run(["import", path.to_str().unwrap(), "--json"]);
        assert!(!proc.crashed(), "import crashed: {}", tail(&proc.stderr));
        let keys = sorted_ids(&m);
        assert!(
            keys.is_empty() || keys == ["ex-x", "ex-y"],
            "{keys:?}: import rc {} {}",
            proc.returncode(),
            proc.stdout.trim()
        );
        assert!(!m.ready().contains("ex-y"));
        // the same file imported once per run stays idempotent
        let path = write_records(&sandbox, "once.jsonl", &one);
        let imported = m.bead(["import", path.to_str().unwrap()]).unwrap();
        let expected = if keys.is_empty() { 2 } else { 0 };
        assert_eq!(
            imported["imported"].as_i64(),
            Some(expected),
            "{imported:?}"
        );
        assert_eq!(sorted_ids(&m), ["ex-x", "ex-y"]);
    }
}
