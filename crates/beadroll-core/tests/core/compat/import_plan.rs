//! Import plans keep record order, dependency metadata and validation diagnostics.

use beadroll_core::id::EventId;
use beadroll_core::import::dry_run::problems;
use beadroll_core::import::{Step, Target, plan_import, read_records};
use beadroll_core::json::Value;
use beadroll_core::state::State;
use beadroll_test_support::events::{Event, id, state};
use beadroll_test_support::json;

fn tracker(dangling: Value) -> State {
    state(&[
        Event::new(&id(1), "init")
            .data(json!({"prefix": "ex"}))
            .json(),
        Event::new(&id(2), "create")
            .deps(&[&id(1)])
            .data(json!({
                "key": "ex-known", "fields": {"title": "known"},
                "imported": true, "dangling": dangling
            }))
            .json(),
    ])
}

#[test]
fn targets_and_metadata_follow_record_and_dependency_order() {
    let state = tracker(json!([]));
    let text = concat!(
        r#"{"id":"ex-z","title":"z","dependencies":["#,
        r#"{"depends_on_id":"ex-a","type":null,"created_by":"first","created_at":"then","metadata":{"n":1},"thread_id":"t"},"#,
        r#"{"depends_on_id":"ex-known","type":"relates-to"},"#,
        r#"{"depends_on_id":"ex-a"},"#,
        r#"{"depends_on_id":"ex-missing","type":"","created_by":"waiting"}]}"#,
        "\n",
        r#"{"id":"ex-known","title":"already here"}"#,
        "\n",
        r#"{"id":"ex-a","title":"a","dependencies":[{"depends_on_id":"ex-z","type":""}]}"#,
        "\n",
    );
    assert!(problems(&state, text, "f").is_empty());
    let plan = plan_import(&state, &read_records(text, "f").unwrap()).unwrap();
    assert_eq!(plan.imported, 2);
    let [Step::Create(z), Step::Create(a), additions @ ..] = plan.steps.as_slice() else {
        panic!("creates must precede dependencies: {plan:?}");
    };
    assert_eq!(z["key"], Value::from("ex-z"));
    assert_eq!(a["key"], Value::from("ex-a"));
    assert_eq!(
        z["dangling"],
        json!([{"depends_on_id": "ex-missing", "type": "blocks", "created_by": "waiting"}])
    );
    assert!(!a.contains_key("dangling"));
    let additions: Vec<_> = additions
        .iter()
        .map(|step| match step {
            Step::DepAdd { issue, to, data } => (*issue, *to, Value::from(data.clone())),
            _ => panic!("expected a dependency: {step:?}"),
        })
        .collect();
    assert_eq!(
        additions,
        [
            (
                0,
                Target::Created(1),
                json!({"type": "blocks", "meta": {
                    "created_at": "then", "created_by": "first", "metadata": {"n": 1}, "thread_id": "t"
                }, "imported": true})
            ),
            (
                0,
                Target::Issue(EventId::parse(&id(2)).unwrap()),
                json!({"type": "relates-to", "meta": {}, "imported": true})
            ),
            (
                0,
                Target::Created(1),
                json!({"type": "blocks", "meta": {}, "imported": true})
            ),
            (
                1,
                Target::Created(0),
                json!({"type": "blocks", "meta": {}, "imported": true})
            ),
        ]
    );
}

#[test]
fn reimport_matches_key_and_normalized_type_and_keeps_retraction_order() {
    let state = tracker(json!([
        {"depends_on_id": "ex-z", "type": "blocks"},
        {"depends_on_id": "ex-shared", "type": "blocks"},
        {"depends_on_id": "ex-a", "type": "blocks"},
        {"depends_on_id": "ex-shared", "type": "relates-to"},
        {"depends_on_id": "ex-null", "type": "blocks"},
        {"depends_on_id": "ex-empty", "type": "blocks"}
    ]));
    let text = concat!(
        "{\"id\":\"ex-new\",\"title\":\"new\"}\n",
        "{\"id\":\"ex-known\",\"dependencies\":[",
        "{\"depends_on_id\":\"ex-shared\"},{\"depends_on_id\":\"ex-shared\",\"type\":\"\"},",
        "{\"depends_on_id\":\"ex-null\",\"type\":null},{\"depends_on_id\":\"ex-empty\",\"type\":\"\"}]}\n",
    );
    let plan = plan_import(&state, &read_records(text, "f").unwrap()).unwrap();
    assert_eq!(plan.imported, 1);
    let [removals @ .., Step::Create(new)] = plan.steps.as_slice() else {
        panic!("retractions must precede creates: {plan:?}");
    };
    assert_eq!(new["key"], Value::from("ex-new"));
    let removals: Vec<_> = removals
        .iter()
        .map(|step| match step {
            Step::DepRemove { issue, data } => {
                assert_eq!(*issue, EventId::parse(&id(2)).unwrap());
                Value::from(data.clone())
            }
            _ => panic!("expected a retraction: {step:?}"),
        })
        .collect();
    assert_eq!(
        removals,
        [
            json!({"key": "ex-z", "type": "blocks"}),
            json!({"key": "ex-a", "type": "blocks"}),
            json!({"key": "ex-shared", "type": "relates-to"}),
        ]
    );
}

#[test]
fn dry_run_preserves_problem_phases_and_first_record_numbers() {
    let state = tracker(json!([]));
    let text = r#"
{"id":"ex-z","title":"z","priority":"high","dependencies":[{"depends_on_id":"ex-a"}]}
{"id":"ex-known","priority":"ignored"}

{"title":"missing id"}
{"id":"ex-a","title":"a"}
{"id":"ex-z","title":"duplicate z"}
{"id":"ex-a","title":"duplicate a","priority":"low"}
"#;
    let found: Vec<_> = problems(&state, text, "f")
        .into_iter()
        .map(|problem| problem.to_string())
        .collect();
    assert_eq!(
        found,
        [
            "f:3: record has no string 'id'",
            "f:5: record id 'ex-z' repeats record 1",
            "f:6: record id 'ex-a' repeats record 4",
            "record 'ex-z' cannot be imported: field 'priority' has the wrong type",
            "record 'ex-a' cannot be imported: field 'priority' has the wrong type",
        ]
    );
}

/// An export of a history written before writes refused a boolean priority or an estimate out of
/// range is one bead's own import takes back, as a Beads reader does: the priority as its number,
/// the estimate left out, every other field as it was.
#[test]
fn an_export_of_legacy_integers_is_imported_again() {
    let legacy = state(&[
        Event::new(&id(1), "init")
            .data(json!({"prefix": "ex"}))
            .json(),
        Event::new(&id(2), "create")
            .deps(&[&id(1)])
            .data(json!({
                "key": "ex-old",
                "fields": {"title": "old", "priority": true, "estimated_minutes": (-7)}
            }))
            .json(),
    ]);
    let records = legacy.hydrated().unwrap().export_records();
    let exported = &records[0];
    assert_eq!(exported["priority"], json!(1), "{exported:?}");
    assert!(exported.get("estimated_minutes").is_none(), "{exported:?}");
    assert_eq!(exported["title"], json!("old"));
    let text: String = (records.iter())
        .map(|record| {
            beadroll_core::json::write(
                &Value::from(record.clone()),
                beadroll_core::json::Format::COMPACT,
            )
            .unwrap()
                + "\n"
        })
        .collect();
    // into a tracker of its own, which holds none of it yet
    let fresh = state(&[Event::new(&id(9), "init")
        .data(json!({"prefix": "ex"}))
        .json()]);
    assert!(problems(&fresh, &text, "f").is_empty(), "{text}");
    let plan = plan_import(&fresh, &read_records(&text, "f").unwrap()).unwrap();
    assert_eq!(plan.imported, 1);
}
