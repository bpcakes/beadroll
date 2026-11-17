//! What an event has seen is what its causes and its own journal say: never what the journal's
//! order suggests, or where a copy of it was found, and a takeover only supersedes a claim it saw.

use beadroll_core::event::Field;
use beadroll_core::journal::{Journal, JournalOrder};
use beadroll_core::op::IntegerField;
use beadroll_core::replay::replay;
use beadroll_test_support::events::journal;
use beadroll_test_support::{Json, json, json_id};

use super::builders::{actors, event_id, id_of, numbered, serial, uid};
use super::properties::fixture::{Fixture, MISSING, conflict, issue, snapshot};
use super::scoped_damage::row;

mod detached;

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
        !st.full_knowledge().unwrap().saw(event_id(&b), event_id(&a)),
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

/// Q07: an inert placeholder must not invent predecessor ancestry for its tail.
#[test]
fn q07_invalid_placeholder_cannot_bridge_a_broken_machine_chain() {
    let init = numbered::event(1, "init", "seed", &[], None, json!({"prefix": "ex"}));
    let create = numbered::event(
        2,
        "create",
        "seed",
        &[id_of(&init)],
        None,
        json!({"key": "ex-one", "fields": {"title": "one"}}),
    );
    let on = Some(id_of(&create));
    let first = numbered::event(
        3,
        "set",
        "a",
        &[id_of(&create)],
        on,
        json!({"fields": {"priority": 1}}),
    );
    let malformed = numbered::event(
        4,
        "comment",
        "a",
        &[id_of(&create)],
        on,
        json!({"text": "bad"}),
    );
    let Json::Object(mut members) = malformed else {
        panic!("an event is an object")
    };
    members.insert("actor".into(), json!([])); // not an actor: the event is invalid
    let malformed = Json::Object(members);
    let later = numbered::event(
        5,
        "set",
        "a",
        &[id_of(&malformed)],
        on,
        json!({"fields": {"priority": 4}}),
    );
    let state = replay(numbered::journal(&[
        &init, &create, &first, &malformed, &later,
    ]))
    .unwrap();
    assert!(
        !state
            .full_knowledge()
            .unwrap()
            .saw(uid(&later), uid(&first))
    );
    assert_eq!(
        state.issue(uid(&create)).unwrap().priority(),
        IntegerField::from(1)
    );
    assert!(state.chain_breaks().contains_key("a"));
}

/// An identical copy of a machine's event in another machine's journal, read before or after the
/// real one, does not reorder or break the real journal.
#[test]
fn identical_misplaced_duplicate_does_not_reorder_own_journal() {
    let mut f = serial::Fixture::new();
    let (init, create) = f.base();
    let first = f.event(
        "comment",
        "z",
        &[id_of(&create)],
        Some(id_of(&create)),
        json!({"text": "first"}),
    );
    let second = f.event(
        "comment",
        "z",
        &[id_of(&first)],
        Some(id_of(&create)),
        json!({"text": "second"}),
    );
    let mut results = Vec::new();
    for misplaced_first in [false, true] {
        let mut journal = Journal::new();
        journal.add(init.clone(), "a", "", JournalOrder::Sequenced);
        journal.add(create.clone(), "a", "", JournalOrder::Sequenced);
        if misplaced_first {
            journal.add(second.clone(), "b", "", JournalOrder::Sequenced);
        }
        journal.add(first.clone(), "z", "", JournalOrder::Sequenced);
        journal.add(second.clone(), "z", "", JournalOrder::Sequenced);
        if !misplaced_first {
            journal.add(second.clone(), "b", "", JournalOrder::Sequenced);
        }
        journal.settle();
        let state = replay(journal).unwrap();
        results.push((serial::texts(&state, &create), state.chain_breaks().clone()));
    }
    assert_eq!(
        results[0], results[1],
        "misplaced identical duplicate reordered the actual owner journal"
    );
    assert_eq!(results[0].0, ["first", "second"]);
    assert!(results[0].1.is_empty());
}

#[test]
fn identical_misplaced_duplicate_does_not_change_broken_chain_result() {
    let mut f = actors::Fixture::new();
    let (init, create) = f.base();
    let uid = json_id(&create);
    let first = f.event(
        "set",
        "a",
        &[&uid],
        actors::Target::Named(&uid),
        json!({"fields": {"title": "first"}}),
    );
    let second = f.event(
        "set",
        "a",
        &[&uid],
        actors::Target::Named(&uid),
        json!({"fields": {"title": "second"}}),
    );
    let projection =
        |misplaced_first: bool| {
            let mut rows = vec![(&init, "a"), (&create, "a")];
            if misplaced_first {
                rows.push((&second, "b"));
            }
            rows.extend([(&first, "a"), (&second, "a")]);
            if !misplaced_first {
                rows.push((&second, "b"));
            }
            let state = actors::replayed(&rows);
            let issue = state.issue_named(&uid).unwrap();
            let conflicts = state.conflicts(issue.hydrated().unwrap()).into_iter().map(
                |(field, alternatives)| {
                    let alternatives = alternatives
                        .iter()
                        .map(|a| Json::from(a.to_value()))
                        .collect::<Vec<_>>();
                    (field.name().to_owned(), Json::Array(alternatives))
                },
            );
            let breaks = state.chain_breaks().iter().map(|(replica, at)| {
                let at = at
                    .iter()
                    .map(|n| json!(i64::try_from(*n).unwrap()))
                    .collect::<Vec<_>>();
                (replica.clone(), Json::Array(at))
            });
            json!({
                "title": (issue.title()),
                "conflicts": (Json::Object(Box::new(conflicts.collect()))),
                "integrity": (Json::Array(state.integrity().iter().map(row).collect())),
                "chain_breaks": (Json::Object(Box::new(breaks.collect())))
            })
        };
    assert_eq!(projection(true), projection(false));
}

#[test]
fn takeover_must_causally_see_the_claim_it_supersedes() {
    let mut f = actors::Fixture::new();
    let (init, create) = f.base();
    let uid = json_id(&create);
    let claim = f.event(
        "claim",
        "a",
        &[&uid],
        actors::Target::Named(&uid),
        json!({"mode": "online", "generation": 1}),
    );
    let takeover = f.event(
        "claim",
        "b",
        &[&uid],
        actors::Target::Named(&uid),
        json!({
            "mode": "online",
            "generation": 2,
            "takeover": (json_id(&claim)),
            "reason": "forged without seeing predecessor"
        }),
    );
    let close = f.event(
        "close",
        "a",
        &[&json_id(&claim)],
        actors::Target::Named(&uid),
        json!({"claim": (json_id(&claim)), "reason": "done"}),
    );
    let state = actors::replayed(&[
        (&init, "a"),
        (&create, "a"),
        (&claim, "a"),
        (&takeover, "b"),
        (&close, "a"),
    ]);
    let event = |e: &Json| beadroll_core::id::EventId::parse(&json_id(e)).unwrap();
    assert!(
        !state
            .full_knowledge()
            .unwrap()
            .saw(event(&takeover), event(&claim))
    );
    let issue = state.issue_named(&uid).unwrap();
    assert_eq!(issue.status(), "closed");
    assert!(issue.claim().is_none());
    assert!(issue.hydrated().unwrap().stale().is_empty());
}
