//! `Review5Replay`: the fifth independent review (2026-10-07-5-codex), folded in after the fixes.

use beadroll_core::event::payload_problem;
use beadroll_core::journal::Journal;
use beadroll_core::journal::JournalOrder;
use beadroll_core::replay::replay;
use beadroll_test_support::json;

/// E10: a valid event's compatibility extras cannot forge its exported identity/state.
#[test]
fn e10_extra_cannot_override_canonical_export_fields() {
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
    if payload_problem(&create).is_some() {
        return;
    }
    let state = replay(journal).unwrap();
    assert!(state.integrity().is_empty());
    let records = state.export_records();
    let exported = &records[0];
    assert_eq!(exported["id"], json!("ex-real"));
    assert_eq!(exported["status"], json!("tombstone"));
    assert!(
        exported
            .get("comments")
            .is_none_or(|comments| comments.as_array().is_some())
    );
}
