//! Text-bearing reads require evidence for the issue being read, not for the whole tracker.

use beadroll_core::code::Code;
use beadroll_core::event::Field;
use beadroll_core::id::EventId;
use beadroll_core::projection::{decode_state, project};
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_test_support::events::{Event, id, journal};
use beadroll_test_support::json;

fn uid(n: u64) -> EventId {
    EventId::parse(&id(n)).unwrap()
}

fn load(state: &mut State, issue: EventId, bytes: &[u8]) {
    for (uid, place) in state.wanted_texts([issue]) {
        let at = place.at as usize;
        state
            .read_texts(uid, place, &bytes[at..at + place.len as usize])
            .unwrap();
    }
}

fn unloaded_texts_are_errors(state: &State) {
    for n in [1, 2, 3] {
        let issue = state.issue(uid(n)).unwrap();
        let error = issue.hydrated().unwrap_err();
        assert_eq!(error.issue, uid(n));
        assert_eq!(error.code(), Code::Validation);
        assert!(issue.read_field(Field::Description).is_err());
        assert!(issue.read_field(Field::Title).unwrap().is_some());
    }
}

#[test]
fn selective_reads_distinguish_unloaded_texts_from_empty_and_preserve_other_issues() {
    let events = vec![
        Event::new(&id(0), "init")
            .data(json!({"prefix": "ex"}))
            .json(),
        Event::new(&id(1), "create")
            .deps(&[&id(0)])
            .data(json!({
                "key": "ex-one", "fields": {"title": "one", "description": "first"},
                "extra": {"imported-note": "kept"}
            }))
            .json(),
        Event::new(&id(2), "create")
            .deps(&[&id(1)])
            .data(json!({"key": "ex-empty", "fields": {"title": "empty", "description": ""}}))
            .json(),
        Event::new(&id(3), "create")
            .deps(&[&id(2)])
            .data(json!({"key": "ex-other", "fields": {"title": "other", "notes": "unread"}}))
            .json(),
        Event::new(&id(4), "comment")
            .deps(&[&id(3)])
            .issue(&id(1))
            .data(json!({"text": "a comment"}))
            .json(),
        Event::new(&id(5), "set")
            .deps(&[&id(4)])
            .issue(&id(1))
            .data(json!({"fields": {"description": "left"}}))
            .json(),
        Event::new(&id(6), "set")
            .replica("b")
            .deps(&[&id(4)])
            .issue(&id(1))
            .data(json!({"fields": {"description": "right"}}))
            .json(),
    ];
    let full = replay(journal(&events)).unwrap();
    assert!(full.integrity().is_empty(), "{:?}", full.integrity());
    let (encoded, _) = project(journal(&events)).unwrap().encode(0);
    let mut partial = decode_state(&encoded.state).unwrap();
    let brief = partial.brief(partial.issue(uid(1)).unwrap());
    unloaded_texts_are_errors(&partial);
    assert_eq!(
        partial.disputed_fields(partial.issue(uid(1)).unwrap()),
        [Field::Description]
    );

    let (wanted, place) = partial.wanted_texts([uid(1)])[0];
    assert!(partial.read_texts(wanted, place, b"truncated").is_err());
    assert!(partial.issue(uid(1)).unwrap().hydrated().is_err());
    load(&mut partial, uid(1), &encoded.texts);
    let issue = partial.issue(uid(1)).unwrap().hydrated().unwrap();
    let expected = full.issue(uid(1)).unwrap().hydrated().unwrap();
    assert_eq!(partial.detail(issue), full.detail(expected));
    assert_eq!(
        partial.details_of(partial.issues()).of(issue),
        full.detail(expected)
    );
    assert_eq!(partial.conflicts(issue), full.conflicts(expected));
    assert_eq!(partial.conflicts(issue).len(), 1);
    assert_eq!(issue.comments().len(), 1);
    assert_eq!(issue.extra().get("imported-note"), Some(&json!("kept")));
    assert_eq!(partial.brief(issue.as_issue()), brief);
    assert!(partial.issue(uid(2)).unwrap().hydrated().is_err());
    assert!(partial.hydrated().is_err());

    load(&mut partial, uid(2), &encoded.texts);
    let empty = partial.issue(uid(2)).unwrap().hydrated().unwrap();
    assert_eq!(empty.field(Field::Description), Some(&json!("")));
    assert_eq!(empty.field(Field::Notes), None);
    assert!(empty.comments().is_empty());
    assert!(partial.issue(uid(3)).unwrap().hydrated().is_err());
    assert_eq!(partial.hydrated().unwrap_err().issue, uid(3));
}

#[test]
fn dependency_summaries_match_full_details_without_loading_texts() {
    let events = vec![
        Event::new(&id(0), "init")
            .data(json!({"prefix": "ex"}))
            .json(),
        Event::new(&id(1), "create")
            .deps(&[&id(0)])
            .data(json!({"key": "ex-one", "imported": true,
                "fields": {"description": "unloaded"},
                "dangling": [{"depends_on_id": "ex-missing", "type": "related"}]}))
            .json(),
        Event::new(&id(2), "create")
            .deps(&[&id(1)])
            .data(json!({"key": "ex-two"}))
            .json(),
        Event::new(&id(3), "dep_add")
            .deps(&[&id(2)])
            .issue(&id(2))
            .data(json!({"to": (id(1)), "type": "blocks"}))
            .json(),
    ];
    let full = replay(journal(&events)).unwrap();
    assert!(full.integrity().is_empty());
    let (encoded, _) = project(journal(&events)).unwrap().encode(0);
    let outline = decode_state(&encoded.state).unwrap();
    for issue in outline.issues() {
        assert!(issue.hydrated().is_err());
        let detail = full.detail(full.issue(issue.uid()).unwrap().hydrated().unwrap());
        let summary = outline.dependency_summary(issue);
        let selected: beadroll_core::json::Object =
            ["dependencies", "dependents", "unresolved_dependencies"]
                .into_iter()
                .filter_map(|name| detail.get(name).map(|value| (name.into(), value.clone())))
                .collect();
        assert_eq!(summary, selected);
    }
}
