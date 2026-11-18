//! A tracker's prefix in its keys: rendered as `str_of` renders it, and `None` for a tracker no
//! `init` named, as Python's f-string made those keys; a bare suffix resolves under it.

use beadroll_core::json::Value;
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_test_support::events::{Event, id, journal};
use beadroll_test_support::json;

/// The tracker an `init` with `init` as its data (none without) and one issue keyed `key` make.
fn tracker(init: Option<Value>, key: &str) -> State {
    let mut events = Vec::new();
    if let Some(data) = init {
        events.push(Event::new(&id(0), "init").data(data).json());
    }
    let create = Event::new(&id(1), "create").data(json!({"key": key, "fields": {"title": "one"}}));
    let create = if events.is_empty() {
        create
    } else {
        create.deps(&[&id(0)])
    };
    events.push(create.json());
    replay(journal(&events)).unwrap()
}

#[test]
fn a_bare_suffix_resolves_under_the_prefix_init_named() {
    let state = tracker(Some(json!({"prefix": "ex"})), "ex-one");
    assert_eq!(state.key_prefix(), "ex");
    assert_eq!(
        state.resolve("one").map(|issue| issue.key()),
        Some("ex-one")
    );
    assert!(state.mint_key("seed", None).unwrap().starts_with("ex-"));
}

#[test]
fn a_prefix_that_is_not_text_is_rendered_as_str_of_renders_it() {
    let state = tracker(Some(json!({"prefix": 5})), "5-one");
    assert_eq!(state.key_prefix(), "5");
    assert_eq!(state.resolve("one").map(|issue| issue.key()), Some("5-one"));
    assert!(state.mint_key("seed", None).unwrap().starts_with("5-"));
}

#[test]
fn a_tracker_no_init_named_keys_its_issues_under_none() {
    let state = tracker(None, "None-one");
    assert_eq!(state.key_prefix(), "None");
    assert_eq!(
        state.resolve("one").map(|issue| issue.key()),
        Some("None-one")
    );
    assert!(state.mint_key("seed", None).unwrap().starts_with("None-"));
}
