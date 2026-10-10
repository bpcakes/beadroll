//! Beads compatibility: Beads JSONL records round-trip through bead.

use std::collections::BTreeSet;

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, json, parse_json};

/// Three Beads records: an epic, its closed child with a dependency on an issue that is not
/// there, a comment and a field Beads does not know, and a tombstone.
fn records() -> Vec<Json> {
    vec![
        json!({"id": "ex-aaa", "title": "Parent", "description": "d", "status": "open",
               "priority": 1, "issue_type": "epic", "created_at": "2026-01-02T03:04:05Z",
               "created_by": "someone", "updated_at": "2026-01-03T00:00:00Z", "source_repo": ".",
               "compaction_level": 0, "original_size": 0, "labels": ["alpha", "beta"]}),
        json!({"id": "ex-aaa.1", "title": "Child", "acceptance_criteria": "it works",
               "status": "closed", "priority": 2, "issue_type": "task",
               "created_at": "2026-01-02T04:00:00Z", "created_by": "someone",
               "updated_at": "2026-01-04T00:00:00Z", "closed_at": "2026-01-04T00:00:00Z",
               "close_reason": "done", "external_ref": "audit:R-1", "source_repo": ".",
               "compaction_level": 0, "original_size": 0, "future_field": {"kept": true},
               "dependencies": [
                   {"issue_id": "ex-aaa.1", "depends_on_id": "ex-aaa", "type": "parent-child",
                    "created_at": "2026-01-02T04:00:00Z", "created_by": "someone",
                    "metadata": "{}", "thread_id": ""},
                   {"issue_id": "ex-aaa.1", "depends_on_id": "ex-gone", "type": "blocks",
                    "created_at": "2026-01-02T04:00:00Z", "created_by": "someone",
                    "metadata": "{}", "thread_id": ""}],
               "comments": [{"id": 41, "issue_id": "ex-aaa.1", "author": "someone",
                             "text": "first", "created_at": "2026-01-03T00:00:00Z"}]}),
        json!({"id": "ex-zzz", "title": "Removed", "status": "tombstone", "priority": 3,
               "issue_type": "bug", "created_at": "2026-01-02T05:00:00Z", "created_by": "someone",
               "updated_at": "2026-01-05T00:00:00Z", "deleted_at": "2026-01-05T00:00:00Z",
               "deleted_by": "someone", "delete_reason": "duplicate", "source_repo": ".",
               "compaction_level": 0, "original_size": 0}),
    ]
}

/// Every line of an export, read as JSON.
fn rows(export: &str) -> Vec<Json> {
    export.lines().map(|row| parse_json(row).unwrap()).collect()
}

/// Beads records survive an import and an export whole, all but the machine-local path, and
/// importing them again adds nothing; an imported comment keeps its id and a new one gets a
/// stable id out of Beads' range; what is ready is what br would offer.
#[test]
fn import_export_round_trips_beads_records() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let records = records();
    let source = sandbox.root().join("issues.jsonl");
    let mut text = String::new();
    for record in &records {
        let mut line = record.as_object().unwrap().clone();
        line.insert(
            "source_repo_path".into(),
            Json::from("/private/machine/path"),
        );
        text += &write(&Json::Object(line), Format::ASCII).unwrap();
        text += "\n";
    }
    std::fs::write(&source, text).unwrap();
    let source = source.to_str().unwrap();
    let init = a
        .bead(["init", "--prefix", "ex", "--import", source])
        .unwrap();
    assert_eq!(init["imported"].as_i64(), Some(3));
    let exported = rows(&a.export());
    assert_eq!(exported, records); // everything but the machine-local path survives
    let again = a.bead(["import", source]).unwrap();
    assert_eq!(again["imported"].as_i64(), Some(0)); // importing again adds nothing
    a.bead(["comments", "add", "ex-aaa.1", "second"]);
    let comments = rows(&a.export())[1]["comments"].clone();
    assert_eq!(comments[0]["id"].as_i64(), Some(41)); // a legacy id is kept as imported
    let new = comments[1]["id"].as_i64().unwrap();
    assert!(new >= 1_000_000, "{new}"); // a new one is stable and out of Beads' range
    assert_eq!(comments[1]["id"], rows(&a.export())[1]["comments"][1]["id"]);
    // the open epic, as in br
    assert_eq!(a.ready(), BTreeSet::from(["ex-aaa".to_owned()]));
    assert_eq!(
        a.bead(["ready", "--type", "task"]),
        Some(Json::Array(Vec::new()))
    );
}
