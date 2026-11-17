//! Clearing a register leaves every query usable: a required one cannot be cleared, an optional one
//! can.

use beadroll_core::event::Field;
use beadroll_core::replay::replay;
use beadroll_test_support::{Json, json};

use super::builders::{id_of, numbered, serial, uid};

/// Q09: required-field clears must be rejected or leave every query usable.
#[test]
fn q09_required_register_clears_remain_queryable() {
    for (offset, field) in (10..).zip([Field::Title, Field::Priority, Field::IssueType]) {
        let init = numbered::event(offset, "init", "seed", &[], None, json!({"prefix": "ex"}));
        let key = format!("ex-{}", field.name());
        let fields = json!({"title": "one", "priority": 2, "issue_type": "task"});
        let create = numbered::event(
            offset + 1,
            "create",
            "seed",
            &[id_of(&init)],
            None,
            json!({"key": (key.as_str()), "fields": (fields.clone())}),
        );
        let mut clear_fields = beadroll_core::json::Object::new();
        clear_fields.insert(field.name().into(), Json::Null);
        let on = Some(id_of(&create));
        let clear = numbered::event(
            offset + 2,
            "set",
            "seed",
            &[id_of(&create)],
            on,
            json!({"fields": (Json::from(clear_fields))}),
        );
        let state = replay(numbered::journal(&[&init, &create, &clear])).unwrap();
        let issue = state.issue(uid(&create)).unwrap();
        // The clear of a required field is refused, with its row, and the field keeps its value.
        assert!(
            state
                .integrity()
                .iter()
                .any(|row| row.event.as_deref() == Some(id_of(&clear))),
            "{field:?}"
        );
        assert_eq!(
            issue.hydrated().unwrap().field(field),
            fields.get(field.name()),
            "{field:?}"
        );
        // The issue stays query-safe.
        state.brief(issue);
        state.line(issue);
        state.hydrated().unwrap().export_records();
    }
}

/// Clearing optional registers is valid, and leaves every query and the export working.
#[test]
fn optional_register_clears_remain_queryable() {
    let mut f = serial::Fixture::new();
    let (init, create) = f.base();
    let fields = json!({"description": null, "design": null, "notes": null, "assignee": null});
    let clear = f.event(
        "set",
        "a",
        &[id_of(&create)],
        Some(id_of(&create)),
        json!({"fields": fields}),
    );
    let state = serial::replayed(&[init, create.clone(), clear]);
    assert!(state.integrity().is_empty());
    state.hydrated().unwrap().export_records();
    let issue = state.issue(uid(&create)).unwrap();
    state.detail(issue.hydrated().unwrap());
    assert_eq!(issue.title(), "survivor");
}
