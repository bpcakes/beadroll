//! `ReplayAuditBugs`, from the first independent audit (AUDIT), 2026-10-06: the defects it found in
//! replay, each pinned. A journal's order invents no ancestry, a duplicated or quarantined event
//! changes nothing by where it is found, an event this bead cannot read is reported and applies
//! nothing, and one malformed event is scoped instead of stopping replay.

use beadroll_core::event::Field;
use beadroll_core::id::EventId;
use beadroll_core::journal::Journal;
use beadroll_core::journal::JournalOrder;
use beadroll_core::replay::replay;
use beadroll_test_support::events::journal;
use beadroll_test_support::{Json, json};

use super::replay_audit_properties::fixture::{Fixture, MISSING, conflict, issue, snapshot};

fn event_id(text: &str) -> EventId {
    EventId::parse(text).unwrap()
}

#[test]
fn non_chain_same_journal_must_not_invent_ancestry() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let a = f.event(
        "set",
        Some(&uid),
        "a",
        &[&uid],
        json!({"fields": {"priority": 1}}),
    );
    let independent = f.event(
        "comment",
        Some(&uid),
        "a",
        &[&uid],
        json!({"text": "did not see a"}),
    );
    let b = f.event(
        "set",
        Some(&uid),
        "b",
        &[&independent],
        json!({"fields": {"priority": 4}}),
    );
    let st = f.state();
    assert!(
        !st.saw(event_id(&b), event_id(&a)),
        "same-journal position invents a causal dependency and erases a concurrent value"
    );
    assert!(conflict(&st, issue(&st, &uid), Field::Priority).is_some());
}

#[test]
fn non_chain_claim_cannot_bypass_pending_predecessor() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    f.event(
        "comment",
        Some(&uid),
        "a",
        &[MISSING],
        json!({"text": "missing predecessor"}),
    );
    f.event(
        "claim",
        Some(&uid),
        "a",
        &[&uid],
        json!({"mode": "online", "generation": 1}),
    );
    let st = f.state();
    assert!(
        issue(&st, &uid).claim().is_none(),
        "later event in malformed journal becomes confirmed while its predecessor is pending"
    );
}

#[test]
fn identical_id_in_different_machine_journals_is_order_independent() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    f.event(
        "set",
        Some(&uid),
        "a",
        &[&uid],
        json!({"fields": {"priority": 1}}),
    );
    f.event(
        "set",
        Some(&uid),
        "b",
        &[&uid],
        json!({"fields": {"priority": 4}}),
    );
    let (a, b) = (f.events[2].clone(), f.events[3].clone());
    let mut views = Vec::new();
    for order in [["a", "b"], ["b", "a"]] {
        let mut j = journal(&f.events[..2]);
        for replica in order {
            // Machine b contains an identical misplaced copy of a, then its own concurrent edit.
            let events = if replica == "a" {
                vec![a.clone()]
            } else {
                vec![a.clone(), b.clone()]
            };
            for event in events {
                j.add(event, replica, "", JournalOrder::Sequenced);
            }
        }
        views.push(snapshot(j));
    }
    assert_eq!(
        views[0], views[1],
        "first physical location of duplicate id changes version-vector identity and conflicts"
    );
}

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
        issue(&st, &uid).field(Field::Notes),
        Some(&json!("valid descendant"))
    );
}

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
    let before = issue(&f.state(), &uid).field(Field::UpdatedAt).cloned();
    f.event(
        "future-op",
        Some(&uid),
        "a",
        &[&uid],
        json!({"important": "not understood"}),
    );
    let st = f.state();
    assert_eq!(issue(&st, &uid).field(Field::UpdatedAt).cloned(), before);
    assert!(!st.integrity().is_empty());
}

#[test]
fn bad_event_schema_has_scoped_damage() {
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
        // A panic in replay fails the test too; one that finishes must report the damage.
        if f.state().integrity().is_empty() {
            damaged.push((name, "not reported in integrity"));
        }
    }
    assert_eq!(
        damaged,
        Vec::<(&str, &str)>::new(),
        "one malformed event crashes replay or silently waits instead of being scoped and reported"
    );
}

#[test]
fn second_parent_blockers_hold_child() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let first = f.create("au-free-parent");
    let second = f.create("au-blocked-parent");
    let gate = f.create("au-gate");
    for (from, to, kind) in [
        (&uid, &first, "parent-child"),
        (&uid, &second, "parent-child"),
        (&second, &gate, "blocks"),
    ] {
        let last = f.last();
        f.event(
            "dep_add",
            Some(from),
            "seed",
            &[&last],
            json!({"to": (to.as_str()), "type": kind}),
        );
    }
    let st = f.state();
    let blockers: Vec<&str> = st
        .open_blockers(issue(&st, &uid))
        .iter()
        .map(|i| i.key())
        .collect();
    assert_eq!(blockers, ["au-gate"]);
}
