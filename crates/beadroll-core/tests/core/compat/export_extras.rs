//! What an export says of an issue comes from bead's own fields: a create's compatibility extras
//! cannot override its id, status or comments.

use beadroll_core::event::payload_problem;
use beadroll_core::journal::{Journal, JournalOrder};
use beadroll_core::replay::replay;
use beadroll_test_support::json;

/// E10: a valid event's compatibility extras cannot forge its exported identity/state.
#[test]
fn extra_cannot_override_canonical_export_fields() {
    let init_id = "019a4444-4444-7000-8000-000000000001";
    let create_id = "019a4444-4444-7000-8000-000000000002";
    let init = json!({
        "v": 1, "id": init_id, "replica": "peer", "deps": [],
        "actor": {"agent": "test", "thread": "peer"}, "op": "init",
        "issue": init_id, "data": {"prefix": "ex", "name": "test"}
    });
    let create = json!({
        "v": 1, "id": create_id, "replica": "peer", "deps": [init_id],
        "actor": {"agent": "test", "thread": "peer"}, "op": "create",
        "issue": create_id,
        "data": {
            "key": "ex-real",
            "fields": {"title": "real", "status": "tombstone", "priority": 2, "issue_type": "task"},
            "labels": [], "comments": [],
            "extra": {"id": "ex-spoof", "status": "open", "comments": "forged"}
        }
    });
    let mut journal = Journal::new();
    journal.add(init, "peer", "", JournalOrder::Sequenced);
    journal.add(create.clone(), "peer", "", JournalOrder::Sequenced);
    assert_eq!(payload_problem(&create), None);
    let state = replay(journal).unwrap();
    assert!(state.integrity().is_empty());
    let records = state.hydrated().unwrap().export_records();
    let exported = &records[0];
    assert_eq!(exported["id"], json!("ex-real"));
    assert_eq!(exported["status"], json!("tombstone"));
    assert!(
        exported
            .get("comments")
            .is_none_or(|comments| comments.as_array().is_some())
    );
}

/// A deleted issue exports its deletion (`deleted_at`, `deleted_by`, `delete_reason`); reopened, it
/// is alive again, and its record says nothing of the delete a Beads reader would take it for.
#[test]
fn a_reopened_issue_exports_no_deletion() {
    let id = |n: u64| format!("019a4444-4444-7000-8000-{n:012x}");
    let event = |n: u64, op: &str, issue: u64, data: beadroll_test_support::Json| {
        let deps: Vec<beadroll_test_support::Json> = (1..n)
            .last()
            .map(|before| json!(id(before)))
            .into_iter()
            .collect();
        json!({
            "v": 1, "id": (id(n)), "replica": "peer", "deps": (beadroll_test_support::Json::Array(deps)),
            "actor": {"agent": "test", "thread": "peer"}, "op": op, "issue": (id(issue)), "data": data
        })
    };
    let history = [
        event(1, "init", 1, json!({"prefix": "ex", "name": "test"})),
        event(
            2,
            "create",
            2,
            json!({"key": "ex-one", "fields": {"title": "one"}}),
        ),
        event(3, "delete", 2, json!({"reason": "duplicate"})),
    ];
    let exported = |events: &[beadroll_test_support::Json]| {
        let mut journal = Journal::new();
        for event in events {
            journal.add(event.clone(), "peer", "", JournalOrder::Sequenced);
        }
        let state = replay(journal).unwrap();
        state.hydrated().unwrap().export_records().remove(0)
    };
    let deleted = exported(&history);
    assert_eq!(deleted["status"], json!("tombstone"));
    assert_eq!(deleted["delete_reason"], json!("duplicate"));
    assert!(deleted.get("deleted_at").is_some(), "{deleted:?}");
    let mut reopened = history.to_vec();
    reopened.push(event(4, "reopen", 2, json!({})));
    let alive = exported(&reopened);
    assert_eq!(alive["status"], json!("open"));
    for member in ["deleted_at", "deleted_by", "delete_reason"] {
        assert!(alive.get(member).is_none(), "{member} in {alive:?}");
    }
}
