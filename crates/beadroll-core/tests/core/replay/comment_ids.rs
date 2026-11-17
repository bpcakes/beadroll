//! A comment's exported id survives what arrives later: a legacy import with the same id, or a
//! colliding comment from another machine.

use std::collections::HashMap;

use beadroll_test_support::json;
use sha2::{Digest, Sha256};

use super::builders::{id_of, serial};

/// A legacy comment imported later with the id a comment made here already exports does not renumber
/// that comment.
#[test]
fn new_comment_integer_survives_collision_with_later_legacy_import() {
    let mut f = serial::Fixture::new();
    let (init, create) = f.base();
    let comment = f.event(
        "comment",
        "a",
        &[id_of(&create)],
        Some(id_of(&create)),
        json!({"text": "existing"}),
    );
    let events = vec![init, create, comment.clone()];
    let before = serial::replayed(&events)
        .hydrated()
        .unwrap()
        .export_records()[0]["comments"][0]["id"]
        .clone();
    let legacy = json!({"legacy_id": (before.clone()), "author": "legacy", "text": "legacy", "created_at": "2020-01-01T00:00:00Z"});
    let data = json!({"key": "ex-two", "fields": {"title": "imported"}, "comments": [legacy], "imported": true});
    let other = f.event("create", "a", &[id_of(&comment)], None, data);
    let after = serial::replayed(&[events, vec![other]].concat());
    let records = after.hydrated().unwrap().export_records();
    let record = records
        .iter()
        .find(|r| r["id"].as_str() == Some("ex-one"))
        .unwrap();
    let existing = &record["comments"][0]["id"];
    assert_eq!(
        *existing, before,
        "later imported legacy ID renumbers an already-exported bead comment"
    );
}

/// Two valid comments whose ids collided under the retired seven-digit formula: the comment
/// exported first keeps its id when the other arrives from another machine.
#[test]
fn valid_colliding_comments_keep_exported_ids_when_other_machine_arrives() {
    let mut f = serial::Fixture::new();
    let (init, create) = f.base();
    let mut seen: HashMap<u64, String> = HashMap::new();
    let mut pair = None;
    for n in 0..100_000u64 {
        let id = format!("019a1111-2222-7000-8000-{n:012x}");
        let digest = Sha256::digest(id.as_bytes());
        let head = u64::from(u32::from_be_bytes([
            digest[0], digest[1], digest[2], digest[3],
        ]));
        let number = 1_000_000 + head % 9_000_000;
        if let Some(earlier) = seen.get(&number) {
            pair = Some((earlier.clone(), id));
            break;
        }
        seen.insert(number, id);
    }
    let (earlier_id, later_id) = pair.expect("deterministic fixture should find a hash collision");
    let on = Some(id_of(&create));
    let earlier = serial::built(
        &earlier_id,
        "comment",
        "b",
        &[id_of(&create)],
        on,
        json!({"text": "arrives later"}),
    );
    let later = serial::built(
        &later_id,
        "comment",
        "c",
        &[id_of(&create)],
        on,
        json!({"text": "already exported"}),
    );
    let state = serial::replayed(&[init.clone(), create.clone(), later.clone()]);
    let before = state.hydrated().unwrap().export_records()[0]["comments"][0]["id"].clone();
    let state = serial::replayed(&[init, create, later, earlier]);
    assert!(state.integrity().is_empty());
    let after = serial::exported_comment(&state, "ex-one", "already exported");
    assert_eq!(
        after, before,
        "valid comments ({earlier_id}, {later_id}) collide and remote arrival renumbers existing export"
    );
}
