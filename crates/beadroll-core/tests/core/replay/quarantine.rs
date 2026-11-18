//! An event replay cannot apply, quarantined, malformed or inert, keeps its damage to itself: it
//! applies nothing, holds up nothing it should not, and every query goes on.

use std::collections::BTreeSet;

use beadroll_core::event::Field;
use beadroll_core::journal::{Journal, JournalOrder};
use beadroll_core::replay::replay;
use beadroll_test_support::events::journal;
use beadroll_test_support::{Json, json, json_id};

use super::builders::{actors, id_of, serial, uid};
use super::properties::fixture::{Fixture, MISSING, issue, snapshot};

#[test]
fn quarantined_takeover_cannot_supersede_a_valid_close() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let claim = f.event(
        "claim",
        Some(&uid),
        "a",
        &[&uid],
        json!({"mode": "online", "generation": 1}),
    );
    let takeover = f.event(
        "claim",
        Some(&uid),
        "b",
        &[&claim],
        json!({"mode": "online", "takeover": (claim.as_str()), "generation": 2}),
    );
    f.event(
        "close",
        Some(&uid),
        "a",
        &[&claim],
        json!({"claim": (claim.as_str())}),
    );
    let original = f
        .events
        .iter()
        .find(|e| e.get("id").and_then(Json::as_str) == Some(&takeover))
        .unwrap()
        .clone();
    let mut alternate = original.clone();
    if let Json::Object(members) = &mut alternate {
        members.insert("op".into(), json!("comment"));
        members.insert("data".into(), json!({"text": "conflicting payload"}));
    }
    let others: Vec<Json> = f
        .events
        .iter()
        .filter(|e| e.get("id").and_then(Json::as_str) != Some(&takeover))
        .cloned()
        .collect();
    let mut journals: Vec<Journal> = Vec::new();
    for variants in [[&original, &alternate], [&alternate, &original]] {
        let mut j = journal(&others);
        for variant in variants {
            j.add(variant.clone(), "b", "", JournalOrder::Sequenced);
        }
        journals.push(j);
    }
    assert_eq!(
        snapshot(journals[0].clone()),
        snapshot(journals[1].clone()),
        "same two quarantined payloads produce different ownership when enumeration reverses"
    );
    for j in journals {
        let st = replay(j).unwrap();
        assert_eq!(st.integrity().len(), 1);
        assert!(
            issue(&st, &uid).claim().is_none(),
            "quarantined claim still participates in superseded pre-pass"
        );
        assert_eq!(issue(&st, &uid).status(), "closed");
    }
}

#[test]
fn quarantined_event_with_missing_cause_does_not_hold_descendants() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let bad = f.event(
        "comment",
        Some(&uid),
        "a",
        &[MISSING],
        json!({"text": "first payload"}),
    );
    f.event(
        "set",
        Some(&uid),
        "a",
        &[&bad],
        json!({"fields": {"notes": "valid descendant"}}),
    );
    let mut j = f.journal();
    let mut variant = f.events[2].clone();
    if let Json::Object(members) = &mut variant {
        members.insert("deps".into(), json!([(uid.as_str())]));
        members.insert("data".into(), json!({"text": "second payload"}));
    }
    j.add(variant, "b", "", JournalOrder::Sequenced);
    let st = replay(j).unwrap();
    assert!(
        st.pending().is_empty(),
        "quarantined event and descendant wait on first payload missing cause"
    );
    assert_eq!(
        issue(&st, &uid).hydrated().unwrap().field(Field::Notes),
        Some(&json!("valid descendant"))
    );
}

#[test]
fn bad_event_schema_has_scoped_damage() {
    // the control: the same comment, well formed, is applied, so a malformed one that is not was
    // left out
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    f.event(
        "comment",
        Some(&uid),
        "a",
        &[&uid],
        json!({"text": "sound"}),
    );
    let st = f.state();
    assert!(st.integrity().is_empty());
    assert_eq!(issue(&st, &uid).hydrated().unwrap().comments().len(), 1);

    let mut damaged = Vec::new();
    for name in [
        "json-array",
        "missing-id",
        "missing-op",
        "missing-replica",
        "bad-id",
        "deps-number",
        "deps-dict",
        "data-array",
    ] {
        let mut f = Fixture::new();
        let uid = f.issue.clone();
        let e = f.event(
            "comment",
            Some(&uid),
            "a",
            &[&uid],
            json!({"text": "malformed"}),
        );
        if name == "json-array" {
            *f.events.last_mut().unwrap() = json!([]);
        } else if let Some(member) = name.strip_prefix("missing-") {
            f.edit(&e, |event| drop(event.shift_remove(member)));
        } else {
            let (member, value) = match name {
                "bad-id" => ("id", json!("not-a-uuid")),
                "deps-number" => ("deps", json!([1])),
                "deps-dict" => ("deps", json!([{}])),
                _ => ("data", json!(["invalid"])),
            };
            f.edit(&e, |event| drop(event.insert(member.into(), value)));
        }
        // A panic in replay fails the test too; one that finishes must report the damage, and
        // keep it to the one event: nothing waits on it, and its issue reads as before, without it.
        let st = f.state();
        let comments =
            (st.issue_named(&uid)).map(|issue| issue.hydrated().unwrap().comments().len());
        if st.integrity().is_empty() {
            damaged.push((name, "not reported in integrity"));
        } else if !st.pending().is_empty() {
            damaged.push((name, "waits as pending"));
        } else if comments.is_none() {
            damaged.push((name, "its issue is lost"));
        } else if comments != Some(0) {
            damaged.push((name, "applied to its issue"));
        }
    }
    assert_eq!(
        damaged,
        Vec::<(&str, &str)>::new(),
        "one malformed event crashes replay or silently waits instead of being scoped and reported"
    );
}

#[test]
fn missing_issue_creates_cannot_alias_and_overwrite() {
    let mut f = actors::Fixture::new();
    let init = f.event(
        "init",
        "a",
        &[],
        actors::Target::Itself,
        json!({"prefix": "ex"}),
    );
    let first = f.event(
        "create",
        "a",
        &[&json_id(&init)],
        actors::Target::Omitted,
        json!({"key": "ex-first", "fields": {"title": "first"}}),
    );
    let second = f.event(
        "create",
        "b",
        &[&json_id(&first)],
        actors::Target::Omitted,
        json!({"key": "ex-second", "fields": {"title": "second"}}),
    );
    let state = actors::replayed(&[(&init, "a"), (&first, "a"), (&second, "b")]);
    // The malformed creates are rejected, each with its row: they never share a missing UID and
    // overwrite (issues are keyed by event id here, so none can).
    assert!(state.issue_named(&json_id(&first)).is_none());
    assert!(state.issue_named(&json_id(&second)).is_none());
    assert!(state.integrity().len() >= 2, "{:?}", state.integrity());
}

#[test]
fn quarantined_common_descendant_dependency_is_scoped() {
    let mut f = actors::Fixture::new();
    let (init, create) = f.base();
    let uid = json_id(&create);
    let quarantined = f.event(
        "comment",
        "b",
        &[],
        actors::Target::Named(&uid),
        json!({"text": "variant one"}),
    );
    let child = f.event(
        "comment",
        "c",
        &[&json_id(&quarantined)],
        actors::Target::Named(&uid),
        json!({"text": "descendant survives"}),
    );
    // The quarantined event names its own descendant as its cause, in both variants.
    let mut quarantined = quarantined;
    if let Json::Object(members) = &mut quarantined {
        members.insert("deps".into(), json!([(json_id(&child))]));
    }
    let mut variant = quarantined.clone();
    if let Json::Object(members) = &mut variant {
        members.insert("data".into(), json!({"text": "variant two"}));
    }
    let state = actors::replayed(&[
        (&init, "a"),
        (&create, "a"),
        (&quarantined, "b"),
        (&child, "c"),
        (&variant, "b"),
    ]);
    assert!(state.pending().is_empty());
    let texts: Vec<Json> = state
        .issue_named(&uid)
        .unwrap()
        .hydrated()
        .unwrap()
        .comments()
        .iter()
        .filter_map(|c| c.get("text"))
        .collect();
    assert_eq!(texts, vec![json!("descendant survives")]);
    assert_eq!(state.integrity().len(), 1);
}

#[test]
fn malformed_embedded_create_comment_is_scoped() {
    let mut f = actors::Fixture::new();
    let (init, create) = f.base();
    let malformed = f.event(
        "create",
        "b",
        &[&json_id(&create)],
        actors::Target::Itself,
        json!({
            "key": "ex-bad",
            "fields": {"title": "malformed"},
            "comments": [{"text": "missing author and created_at"}]
        }),
    );
    let state = actors::replayed(&[(&init, "a"), (&create, "a"), (&malformed, "b")]);
    // The malformed issue is rejected, and its one bad embedded comment cannot disable global
    // export.
    let records = state.hydrated().unwrap().export_records();
    assert!(
        records
            .iter()
            .any(|record| record.get("id") == Some(&json!("ex-one")))
    );
    assert!(state.issue_named(&json_id(&malformed)).is_none());
    assert!(!state.integrity().is_empty());
}

/// A quarantined comment whose variants agree on a cause that loops back to it is released, applies
/// nothing, and does not hold the comment built on it.
#[test]
fn quarantine_agreed_cycle_is_scoped() {
    let mut f = serial::Fixture::new();
    let (init, create) = f.base();
    let bad_id = f.next_id();
    let child = f.event(
        "comment",
        "c",
        &[&bad_id],
        Some(id_of(&create)),
        json!({"text": "surviving child"}),
    );
    let bad = serial::built(
        &bad_id,
        "comment",
        "b",
        &[id_of(&child)],
        Some(id_of(&create)),
        json!({"text": "variant one"}),
    );
    let variant = serial::with(&bad, &["data", "text"], json!("variant two"));
    let state = replay(serial::journal(
        &[init, create.clone(), bad, child, variant],
        &BTreeSet::new(),
    ))
    .unwrap();
    assert!(state.pending().is_empty());
    assert_eq!(serial::texts(&state, &create), ["surviving child"]);
    assert!(!state.integrity().is_empty());
}

/// An id whose second copy has a scalar for its causes is quarantined, applies nothing, and replay
/// goes on.
#[test]
fn quarantine_malformed_scalar_deps_is_scoped() {
    let mut f = serial::Fixture::new();
    let (init, create) = f.base();
    let comment = f.event(
        "comment",
        "b",
        &[id_of(&create)],
        Some(id_of(&create)),
        json!({"text": "ambiguous"}),
    );
    let variant = serial::with(&comment, &["deps"], json!(17));
    let state = serial::replayed(&[init, create.clone(), comment, variant]);
    assert!(serial::texts(&state, &create).is_empty());
    assert!(!state.integrity().is_empty());
}

/// An embedded comment without its author and time makes its create inert, and every query and the
/// export keep working.
#[test]
fn malformed_imported_comment_does_not_break_queries() {
    let mut f = serial::Fixture::new();
    let (init, create) = f.base();
    let create = serial::with(
        &create,
        &["data", "comments"],
        json!([{"text": "missing author and timestamp"}]),
    );
    let state = serial::replayed(&[init, create.clone()]);
    state.hydrated().unwrap().export_records();
    for issue in state.issues() {
        state.detail(issue.hydrated().unwrap());
    }
    assert!(
        !state.integrity().is_empty(),
        "malformed embedded comment must make its event inert"
    );
    assert!(state.issue(uid(&create)).is_none());
}

/// A create that names no issue is inert: no issue is installed under no id.
#[test]
fn create_without_issue_id_is_inert() {
    let mut f = serial::Fixture::new();
    let (init, create) = f.base();
    let Json::Object(mut members) = create else {
        panic!("an event is an object")
    };
    members.shift_remove("issue");
    let state = serial::replayed(&[init, Json::Object(members)]);
    assert!(
        !state.integrity().is_empty(),
        "missing create issue ID must not install inaccessible None-keyed issue"
    );
    assert_eq!(state.issues().count(), 0);
}
