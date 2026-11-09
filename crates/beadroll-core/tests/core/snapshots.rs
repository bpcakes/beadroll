//! Consumer-facing state capabilities and reads tied to one published tree (roll-sb21).

use beadroll_core::id::EventId;
use beadroll_core::journal::{Journal, JournalOrder};
use beadroll_core::json::{self, Format, Value};
use beadroll_core::order::{causal_order, effective_deps};
use beadroll_core::projection::{Extended, Projection, Record, decode_state, project};
use beadroll_core::published::Tree;
use beadroll_core::recap::{Window, unseen};
use beadroll_core::replay::replay;
use beadroll_core::snapshot::Snapshot;
use beadroll_core::state::{KnowledgeScope, State};
use beadroll_test_support::events::{Event, id, journal};
use beadroll_test_support::json;

fn eid(n: u64) -> EventId {
    EventId::parse(&id(n)).unwrap()
}

fn records() -> Vec<Value> {
    vec![
        Event::new(&id(0), "init")
            .data(json!({"prefix": "ex"}))
            .json(),
        Event::new(&id(1), "create")
            .deps(&[&id(0)])
            .data(json!({
                "key": "ex-one", "fields": {"title": "before", "description": "long description"},
                "extra": {"x-origin": "imported"}
            }))
            .json(),
        Event::new(&id(2), "create")
            .deps(&[&id(1)])
            .data(json!({
                "key": "ex-two", "fields": {"title": "two", "notes": "long notes"}
            }))
            .json(),
        Event::new(&id(3), "comment")
            .deps(&[&id(2)])
            .issue(&id(1))
            .data(json!({"text": "a comment"}))
            .json(),
        Event::new(&id(4), "dep_add")
            .deps(&[&id(3)])
            .issue(&id(1))
            .data(json!({"to": (id(2)), "type": "blocks", "meta": {"metadata": "kept"}}))
            .json(),
    ]
}

fn load_texts(state: &mut State, texts: &[u8]) {
    for (uid, place) in state.all_wanted_texts() {
        let at = usize::try_from(place.at).unwrap();
        state
            .read_texts(uid, place, &texts[at..at + place.len as usize])
            .unwrap();
    }
}

#[test]
fn dirty_journals_expose_no_settled_read_until_all_records_are_judged() {
    let first = records().remove(0);
    let conflicting = Event::new(&id(0), "init")
        .data(json!({"prefix": "other"}))
        .json();
    let fresh = records().remove(1);
    let mut read = Journal::new();
    read.add(first, "a", "first", JournalOrder::Sequenced);
    assert_eq!(read.settled_view().events().len(), 1);
    read.add(conflicting, "a", "conflict", JournalOrder::Sequenced);
    read.add(fresh, "a", "fresh", JournalOrder::Sequenced);
    assert!(read.settled().is_none());
    let settled = read.settled_view();
    assert_eq!(settled.events().len(), 2);
    assert!(settled.quarantined().contains_key(&eid(0)));
    assert!(settled.event(eid(1)).is_some());
    let (placed, pending) = causal_order(settled);
    assert_eq!(placed, [eid(0), eid(1)]);
    assert!(pending.is_empty());
    assert_eq!(unseen(settled, Window::All).placed, placed);
    assert_eq!(effective_deps(settled, eid(1)), Some(vec![eid(0)]));
    assert_eq!(effective_deps(settled, eid(99)), None);
    assert!(read.settled().is_some());
}

#[test]
fn complete_export_requires_every_issues_texts_and_failed_loading_grants_nothing() {
    let full = replay(journal(&records())).unwrap();
    assert!(full.integrity().is_empty(), "{:?}", full.integrity());
    let expected = full.hydrated().unwrap().export_records();
    let (encoded, _) = project(journal(&records())).unwrap().encode(0);
    let mut summary = decode_state(&encoded.state).unwrap();
    assert_eq!(summary.knowledge_scope(), KnowledgeScope::Summary);
    assert!(summary.full_knowledge().is_none());
    assert_eq!(summary.hydrated().unwrap_err().issue, eid(1));
    let (uid, place) = summary.all_wanted_texts()[0];
    assert!(summary.read_texts(uid, place, b"damaged").is_err());
    assert!(summary.hydrated().is_err());
    let at = place.at as usize;
    summary
        .read_texts(uid, place, &encoded.texts[at..at + place.len as usize])
        .unwrap();
    assert_eq!(summary.hydrated().unwrap_err().issue, eid(2));
    // Selective readers still have the first issue's detail before the rest are loaded.
    assert_eq!(
        summary
            .resolve("ex-one")
            .unwrap()
            .hydrated()
            .unwrap()
            .comments()
            .len(),
        1
    );
    load_texts(&mut summary, &encoded.texts);
    assert_eq!(summary.hydrated().unwrap().export_records(), expected);
    assert!(
        summary.full_knowledge().is_none(),
        "hydration is not ancestry"
    );
    assert_eq!(summary.clone().knowledge_scope(), KnowledgeScope::Summary);
    assert_eq!(
        expected[0].get("description"),
        Some(&json!("long description"))
    );
    assert_eq!(expected[0].get("x-origin"), Some(&json!("imported")));
    assert_eq!(expected[0]["comments"][0]["text"], json!("a comment"));
    assert_eq!(expected[0]["dependencies"][0]["metadata"], json!("kept"));
}

#[test]
fn projections_never_claim_full_history_even_when_hydrated_or_extended() {
    let mut events = records();
    events.push(
        Event::new(&id(5), "set")
            .deps(&[&id(4)])
            .issue(&id(1))
            .data(json!({"fields": {"title": "after"}}))
            .json(),
    );
    let full = replay(journal(&events)).unwrap();
    let knowledge = full.full_knowledge().unwrap();
    assert_eq!(full.knowledge_scope(), KnowledgeScope::Full);
    assert!(knowledge.holds(eid(3)));
    assert!(knowledge.saw(eid(5), eid(3)));
    assert_eq!(knowledge.events().count(), 6);
    let projection = project(journal(&events)).unwrap();
    assert!(
        !projection.retains(eid(3)),
        "comment ancestry was compacted away"
    );
    assert_eq!(
        projection.state().knowledge_scope(),
        KnowledgeScope::Retained
    );
    assert!(projection.state().hydrated().is_ok());
    assert!(projection.state().full_knowledge().is_none());
    let (encoded, _) = projection.encode(0);
    let mut decoded = Projection::decode(encoded.clone()).unwrap();
    assert_eq!(decoded.state().knowledge_scope(), KnowledgeScope::Retained);
    assert!(decoded.state().hydrated().is_err());
    load_texts(decoded.state_mut(), &encoded.texts);
    assert!(decoded.state().full_knowledge().is_none());
    let next = Event::new(&id(6), "comment")
        .deps(&[&id(5)])
        .issue(&id(1))
        .data(json!({"text": "next"}))
        .json();
    let extended = decoded
        .prepare_extension(vec![Record {
            value: next,
            replica: "a".into(),
            origin: "next".into(),
            sequenced: true,
        }])
        .apply()
        .unwrap();
    let Extended::Applied(extended) = extended else {
        panic!("sequential extension declined")
    };
    assert_eq!(extended.state().knowledge_scope(), KnowledgeScope::Retained);
    assert!(extended.state().full_knowledge().is_none());
    assert!(extended.state().hydrated().is_ok());
    assert!(full.clone().full_knowledge().is_some());
}

fn tree(events: &[Value]) -> Tree {
    let lines: String = events
        .iter()
        .map(|event| format!("{}\n", json::write(event, Format::COMPACT).unwrap()))
        .collect();
    Tree::read(
        [
            (
                "manifest.json".into(),
                b"{\"format\":1,\"workspace\":\"workspace\"}".to_vec(),
            ),
            ("events/a/000001.jsonl".into(), lines.into_bytes()),
        ],
        "workspace",
    )
    .unwrap()
}

#[test]
fn tree_snapshot_binds_activity_export_and_issue_resolution_to_one_read() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Snapshot>();
    let mut events = records();
    let old = tree(&events).snapshot().unwrap();
    let selected = old.recap(Window::All);
    events.push(
        Event::new(&id(5), "set")
            .deps(&[&id(4)])
            .issue(&id(1))
            .data(json!({"fields": {"title": "after"}}))
            .json(),
    );
    let new = tree(&events).snapshot().unwrap();
    let created = selected
        .events()
        .find(|event| event.event().id() == eid(1))
        .unwrap();
    assert_eq!(created.event().issue(), Some(id(1).as_str()));
    assert_eq!(created.issue().unwrap().title(), "before");
    let later_create = new.event(eid(1)).unwrap();
    assert_eq!(later_create.issue().unwrap().title(), "after");
    let activity = selected
        .events()
        .find(|event| event.event().id() == eid(3))
        .unwrap();
    assert_eq!(activity.issue().unwrap().title(), "before");
    assert_eq!(
        activity.state().export_records()[0]["title"],
        json!("before")
    );
    assert_eq!(
        selected.snapshot().state().export_records()[0]["title"],
        json!("before")
    );
    assert_eq!(new.event(eid(3)).unwrap().issue().unwrap().title(), "after");
    assert_eq!(selected.event_ids(), [eid(1), eid(2), eid(3), eid(4)]);
    assert_eq!(selected.placed().count(), 5); // init counts as placed, never activity
    assert_eq!(selected.frontier(), [eid(4)]);
    assert_eq!(
        new.recap(Window::After(selected.frontier())).event_ids(),
        [eid(5)]
    );
    assert_eq!(selected.joined(&[eid(5)]), [eid(4), eid(5)]);
    assert!(!old.knowledge().holds(eid(5)));
    assert!(new.knowledge().saw(eid(5), eid(3)));
    let window = old.recap(Window::Since(eid(0).millis() + 1));
    assert!(window.event_ids().is_empty());
    assert_eq!(window.earlier(), 4);
    assert_eq!(window.frontier(), [eid(4)]);
}

#[test]
fn pending_ancestry_recognizes_only_named_causes_until_the_event_is_placed() {
    let mut events = records();
    events.push(
        Event::new(&id(6), "comment")
            .deps(&[&id(4), &id(5)])
            .issue(&id(1))
            .data(json!({"text": "waiting for a cause"}))
            .json(),
    );
    let pending = Snapshot::from_journal(journal(&events)).unwrap();
    let knowledge = pending.knowledge();
    assert_eq!(pending.state().pending(), [eid(6)]);
    assert!(knowledge.holds(eid(6)));
    assert!(knowledge.events().any(|id| id == eid(6)));
    assert!(!knowledge.who(eid(6)).is_empty());
    assert_eq!(knowledge.who(eid(6)), knowledge.who(eid(4)));
    assert!(knowledge.saw(eid(6), eid(4)));
    assert!(
        knowledge.saw(eid(6), eid(5)),
        "a named cause need not be held"
    );
    assert!(!knowledge.holds(eid(5)));
    assert!(knowledge.saw(eid(4), eid(3)));
    assert!(
        !knowledge.saw(eid(6), eid(3)),
        "a pending event has no clock for its transitive ancestry"
    );

    events.push(
        Event::new(&id(5), "comment")
            .replica("b")
            .deps(&[&id(4)])
            .issue(&id(1))
            .data(json!({"text": "the missing cause"}))
            .json(),
    );
    let placed = Snapshot::from_journal(journal(&events)).unwrap();
    assert!(placed.state().pending().is_empty());
    assert!(placed.knowledge().saw(eid(6), eid(3)));
    assert!(placed.knowledge().saw(eid(6), eid(5)));
    assert!(
        !knowledge.saw(eid(6), eid(3)),
        "the earlier read stays pending"
    );
}

#[test]
fn bound_recap_keeps_pending_and_quarantined_events_in_its_own_read() {
    let mut events = records();
    // The cause arrives after a read that already holds the effect.
    let effect = Event::new(&id(6), "comment")
        .deps(&[&id(5)])
        .issue(&id(1))
        .data(json!({"text": "waiting"}))
        .json();
    events.push(effect);
    let first = Snapshot::from_journal(journal(&events)).unwrap();
    let recap = first.recap(Window::All);
    assert_eq!(
        recap
            .pending()
            .map(|event| event.event().id())
            .collect::<Vec<_>>(),
        [eid(6)]
    );
    assert_eq!(recap.frontier(), [eid(4)]);
    let cause = Event::new(&id(5), "comment")
        .replica("b")
        .deps(&[&id(4)])
        .issue(&id(1))
        .data(json!({"text": "cause"}))
        .json();
    events.push(cause);
    let later = Snapshot::from_journal(journal(&events)).unwrap();
    let next = later.recap(Window::After(recap.frontier()));
    assert_eq!(next.event_ids(), [eid(5), eid(6)]);
    assert_eq!(next.pending().count(), 0);
    assert_eq!(next.joined(recap.frontier()), [eid(6)]);
    assert_eq!(
        recap.pending().count(),
        1,
        "the earlier result stays unchanged"
    );
    // A conflicting copy becomes inert, and the bound reader reports that same judgment.
    events.push(
        Event::new(&id(5), "comment")
            .replica("b")
            .deps(&[&id(4)])
            .issue(&id(1))
            .data(json!({"text": "conflicting cause"}))
            .json(),
    );
    let conflicted = Snapshot::from_journal(journal(&events)).unwrap();
    assert!(conflicted.event(eid(5)).unwrap().quarantined());
    assert!(!later.event(eid(5)).unwrap().quarantined());
    assert!(
        conflicted
            .state()
            .integrity()
            .iter()
            .any(|row| row.event.as_deref() == Some(&id(5)))
    );
    assert_eq!(
        conflicted.recap(Window::After(&[eid(5)])).event_ids(),
        [eid(1), eid(2), eid(3), eid(4), eid(6)]
    );
}
