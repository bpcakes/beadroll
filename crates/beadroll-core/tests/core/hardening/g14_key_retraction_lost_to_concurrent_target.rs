//! G14: a key-based `dep_remove` (the target is not an issue on the remover's machine) only
//! filtered `dangling`; when another machine's concurrent import of the target had the earlier
//! event id, replay had already turned the entry into an edge, so the retraction was lost.

use beadroll_core::journal::Journal;
use beadroll_core::json::Value;
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_test_support::events::Event;
use beadroll_test_support::json;

const INIT: &str = "01900000-0000-7000-8000-000000000000";
const A: &str = "01900000-0000-7000-8000-000000000001";
const EARLY: &str = "01900000-0000-7000-8000-000000000002";
const LATE: &str = "01900000-0000-7000-8000-000000000003";
const ADD: &str = "01900000-0000-7000-8000-000000000004";

fn fields() -> Value {
    json!({"title": "t", "status": "open", "priority": 2, "issue_type": "task"})
}

/// These events, without actors, each added to its writer's journal.
fn replayed(events: Vec<Event>) -> State {
    let mut journal = Journal::new();
    for event in events {
        let event = event.without("actor").json();
        let replica = event["replica"].as_str().unwrap().to_owned();
        journal.add(event, &replica, "", true);
    }
    replay(journal).unwrap()
}

fn init() -> Event {
    Event::new(INIT, "init")
        .replica("m1")
        .without("issue")
        .data(json!({"prefix": "ex", "name": "ex"}))
}

/// ex-a, imported with a dependency on ex-b, which is not an issue yet.
fn import_a() -> Event {
    Event::new(A, "create")
        .replica("m1")
        .deps(&[INIT])
        .data(json!({
            "key": "ex-a",
            "fields": (fields()),
            "imported": true,
            "dangling": [{"depends_on_id": "ex-b", "type": "blocks"}]
        }))
}

fn key_removal(id: &str) -> Event {
    Event::new(id, "dep_remove")
        .replica("m1")
        .deps(&[A])
        .issue(A)
        .data(json!({"key": "ex-b", "type": null}))
}

fn import_b(id: &str) -> Event {
    Event::new(id, "create")
        .replica("m2")
        .deps(&[A])
        .data(json!({"key": "ex-b", "fields": (fields()), "imported": true}))
}

/// ex-a's edges, sorted, and whether anything holds it.
fn project(remove: &str, create_b: &str) -> (Vec<(String, String)>, bool) {
    let state = replayed(vec![
        init(),
        import_a(),
        key_removal(remove),
        import_b(create_b),
    ]);
    let a = state.issue_named(A).unwrap();
    let mut edges: Vec<(String, String)> = a.deps().keys().cloned().collect();
    edges.sort();
    (edges, state.is_blocked(a))
}

/// G14: replay of {import ex-a (dangling ex-b), key dep_remove, concurrent create of ex-b} gives
/// the same edges whichever of the two concurrent events has the smaller id; a concurrent explicit
/// `dep add` that had not seen the removal still keeps its edge.
#[test]
fn replay_does_not_depend_on_the_tie_break_between_the_two_concurrent_events() {
    assert_eq!(project(EARLY, LATE), (vec![], false));
    assert_eq!(
        project(LATE, EARLY),
        (vec![], false),
        "the key-based removal lost because the target's create had the smaller id"
    );
    // Only the import's own tag is taken back: a concurrent explicit `dep add ex-a ex-b` by a writer
    // that had not seen the removal keeps its edge (add-wins).
    let add = Event::new(ADD, "dep_add")
        .replica("m2")
        .deps(&[EARLY])
        .issue(A)
        .data(json!({"to": EARLY, "type": "blocks"}));
    let state = replayed(vec![
        init(),
        import_a(),
        import_b(EARLY),
        add,
        key_removal(LATE),
    ]);
    let edges: Vec<(String, String)> = state
        .issue_named(A)
        .unwrap()
        .deps()
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        edges,
        vec![(EARLY.to_owned(), "blocks".to_owned())],
        "a concurrent explicit dep add was removed by a key-based retraction it had not seen"
    );
}
