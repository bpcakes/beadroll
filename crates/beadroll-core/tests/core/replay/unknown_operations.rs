//! An event this bead cannot read, of an unknown version or operation, is reported and applies
//! nothing, a stand-in's operation included.

use beadroll_core::event::Field;
use beadroll_test_support::{Json, json};

use super::builders::actors;
use super::properties::fixture::{Fixture, issue};
use super::scoped_damage::{id, row};

#[test]
fn unknown_event_version_is_not_applied_silently() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let e = f.event(
        "set",
        Some(&uid),
        "a",
        &[&uid],
        json!({"fields": {"title": "future semantic version"}}),
    );
    f.edit(&e, |event| drop(event.insert("v".into(), json!(999))));
    let st = f.state();
    assert_eq!(issue(&st, &uid).title(), "au-one");
    assert!(!st.integrity().is_empty());
}

#[test]
fn unknown_operation_is_reported_and_does_not_mutate_timestamp() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let before = issue(&f.state(), &uid)
        .hydrated()
        .unwrap()
        .field(Field::UpdatedAt)
        .cloned();
    f.event(
        "future-op",
        Some(&uid),
        "a",
        &[&uid],
        json!({"important": "not understood"}),
    );
    let st = f.state();
    assert_eq!(
        issue(&st, &uid)
            .hydrated()
            .unwrap()
            .field(Field::UpdatedAt)
            .cloned(),
        before
    );
    assert!(!st.integrity().is_empty());
}

/// AS01: an event may name the operation one of the journal's own stand-ins carries (`quarantined`,
/// `invalid`). It is an unknown operation like any other: reported under its issue and applied
/// nowhere, never taken for a stand-in that was reported as it was loaded.
#[test]
fn an_event_naming_a_stand_ins_operation_is_an_unknown_operation() {
    for op in ["frobnicate", "quarantined", "invalid"] {
        let mut f = actors::Fixture::new();
        let (init, create) = f.base();
        let uid = id(&create);
        let odd = f.event(op, "a", &[&uid], actors::Target::Named(&uid), json!({}));
        let state = actors::replayed(&[(&init, "a"), (&create, "a"), (&odd, "a")]);
        let rows: Vec<Json> = state.integrity().iter().map(row).collect();
        let expected = json!({
            "event": (id(&odd)),
            "issue": "ex-one",
            "problem": (format!("unknown operation '{op}'"))
        });
        assert_eq!(rows, [expected], "{op}");
        assert_eq!(state.resolve("ex-one").unwrap().title(), "survivor", "{op}");
    }
}
