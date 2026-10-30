//! Validation regressions U01–U04 and U06–U10 (`docs/FINDINGS.md`).
//! Journal interpretation must be deterministic, and no accepted create payload may break a
//! query.

mod lines;

use beadroll_core::event::payload_problem;
use beadroll_core::id::EventId;
use beadroll_core::journal::Journal;
use beadroll_core::journal::JournalOrder;
use beadroll_core::json::Object;
use beadroll_core::replay::replay;
use beadroll_core::state::{Integrity, Issue, Rename, State};
use beadroll_test_support::events::Event;
use beadroll_test_support::{Json, json};

/// Event `number`: id `019a5555-5555-7000-8000-<number>`, about `issue` (itself when
/// `None`).
fn event(
    number: u64,
    op: &str,
    replica: &str,
    deps: &[&str],
    issue: Option<&str>,
    data: Json,
) -> Json {
    let id = format!("019a5555-5555-7000-8000-{number:012x}");
    let event = Event::new(&id, op).replica(replica).deps(deps).data(data);
    match issue {
        Some(issue) => event.issue(issue).json(),
        None => event.json(),
    }
}

fn id_of(event: &Json) -> &str {
    event["id"].as_str().unwrap()
}

fn uid(event: &Json) -> EventId {
    EventId::parse(id_of(event)).unwrap()
}

/// `event` with its member `name` set to `value`, in its place.
fn with(event: &Json, name: &str, value: Json) -> Json {
    let mut members = event.as_object().unwrap().clone();
    members.insert(name.to_owned(), value);
    Json::from(members)
}

/// `{**base, **extra}`.
fn merged(base: &Json, extra: &Json) -> Json {
    let mut members = base.as_object().unwrap().clone();
    members.extend(extra.as_object().unwrap().clone());
    Json::from(members)
}

fn problems(state: &State) -> Vec<&str> {
    state
        .integrity()
        .iter()
        .map(|row| row.problem.as_str())
        .collect()
}

/// U06: \d matched any Unicode digit, so a renamed copy of `x.٣` became `x.3` and a child of a parent
/// with child `p.١` counted on from 1; a key's number is ASCII digits only.
#[test]
fn u06_key_numbers_are_ascii_digits() {
    let init = event(1, "init", "seed", &[], None, json!({"prefix": "ex"}));
    let parent = event(
        2,
        "create",
        "seed",
        &[id_of(&init)],
        None,
        json!({"key": "ex-p", "fields": {"title": "p"}}),
    );
    let child = event(
        3,
        "create",
        "seed",
        &[id_of(&parent)],
        None,
        json!({"key": "ex-p.١", "fields": {"title": "c"}}),
    );
    let first = event(
        4,
        "create",
        "a",
        &[id_of(&init)],
        None,
        json!({"key": "x.٣", "fields": {"title": "one"}}),
    );
    let second = event(
        5,
        "create",
        "b",
        &[id_of(&init)],
        None,
        json!({"key": "x.٣", "fields": {"title": "two"}}),
    );
    let mut journal = Journal::new();
    for event in [&init, &parent, &child, &first, &second] {
        let replica = event["replica"].as_str().unwrap().to_owned();
        journal.add(event.clone(), &replica, "", JournalOrder::Sequenced);
    }
    let state = replay(journal).unwrap();
    let renamed = state.issue(uid(&second)).unwrap().key().to_owned();
    assert!(
        renamed.starts_with("x.٣") && renamed.chars().count() > "x.٣".chars().count(),
        "{renamed}"
    );
    let rename = Rename {
        from: "x.٣".into(),
        to: renamed,
        replica: "b".into(),
        uid: uid(&second),
    };
    assert_eq!(state.renames(), [rename]);
    let parent = state.issue(uid(&parent)).unwrap();
    assert_eq!(state.mint_key("seed", Some(parent)).unwrap(), "ex-p.1");
}

/// U07: three malformed payloads must fail validation with stable diagnostics rather than fail
/// during replay.
#[test]
fn u07_payloads_that_reached_the_last_resort_are_refused_by_name() {
    let init = event(1, "init", "seed", &[], None, json!({"prefix": "ex"}));
    let target = event(
        2,
        "create",
        "seed",
        &[id_of(&init)],
        None,
        json!({"key": "ex-t", "fields": {"title": "t"}}),
    );
    let long = format!("x.{}", "1".repeat(4300));
    let taken = event(
        3,
        "create",
        "seed",
        &[id_of(&target)],
        None,
        json!({"key": (long.as_str()), "fields": {"title": "x"}}),
    );
    let rows = [
        (
            "dep_add event has a malformed 'meta'",
            json!({"to": (id_of(&target)), "meta": [1]}),
        ),
        (
            "dep_remove event has a malformed 'key'",
            json!({"to": (id_of(&target)), "key": ["ex-t"]}),
        ),
    ];
    let mut journal = Journal::new();
    for event in [&init, &target, &taken] {
        journal.add(event.clone(), "seed", "", JournalOrder::Sequenced);
    }
    let mut previous = id_of(&taken).to_owned();
    for (number, (problem, data)) in (4..).zip(&rows) {
        let op = problem.split_whitespace().next().unwrap();
        // One chain: each builds on the last.
        let event = event(
            number,
            op,
            "seed",
            &[&previous],
            Some(id_of(&target)),
            data.clone(),
        );
        assert_eq!(payload_problem(&event).as_deref(), Some(*problem));
        previous = id_of(&event).to_owned();
        journal.add(event, "seed", "", JournalOrder::Sequenced);
    }
    let state = replay(journal).unwrap();
    let mut found = problems(&state);
    found.sort_unstable();
    let mut expected = vec![rows[0].0, rows[1].0, "create event has no valid 'key'"];
    expected.sort_unstable();
    assert_eq!(found, expected);
    assert!(
        !found
            .iter()
            .any(|problem| problem.contains("could not be applied"))
    );
    // The original admission limit leaves a digit for a duplicate's carry.
    let nines = format!("x.{}", "9".repeat(4300));
    let refused = event(
        9,
        "create",
        "a",
        &[],
        None,
        json!({"key": (nines.as_str())}),
    );
    assert_eq!(
        payload_problem(&refused).as_deref(),
        Some("create event has no valid 'key'")
    );
    let nines = format!("x.{}", "9".repeat(4299));
    let mut journal = Journal::new();
    for (number, replica) in [(10, "a"), (11, "b")] {
        journal.add(
            event(
                number,
                "create",
                replica,
                &[],
                None,
                json!({"key": (nines.as_str())}),
            ),
            replica,
            "",
            JournalOrder::Sequenced,
        );
    }
    let state = replay(journal).unwrap();
    let keys: Vec<&str> = state.issues().map(Issue::key).collect();
    assert_eq!(keys, [nines, format!("x.1{}", "0".repeat(4299))]);
    assert!(state.integrity().is_empty());
}

/// U08: a create's comments are imported ones. One naming a uid or a where, which only a comment made
/// here has, broke `show` or `export` (a where that is not an object, a uid that is not a string) or
/// exported as a comment made here; it is refused.
#[test]
fn u08_imported_comments_carry_neither_a_uid_nor_a_where() {
    let comment = json!({"text": "hi", "author": null, "created_at": null});
    let extras = [
        json!({"uid": 5}),
        json!({"uid": "x"}),
        json!({"uid": null}),
        json!({"where": null}),
        json!({"where": {"branch": "b"}}),
    ];
    for extra in &extras {
        let create = event(
            2,
            "create",
            "a",
            &[],
            None,
            json!({"key": "ex-1", "comments": [(merged(&comment, extra))]}),
        );
        assert_eq!(
            payload_problem(&create).as_deref(),
            Some("create event has a malformed comment"),
            "{extra:?}"
        );
    }
    let init = event(1, "init", "a", &[], None, json!({"prefix": "ex"}));
    let numbered = merged(&comment, &json!({"legacy_id": 7}));
    let kept = event(
        2,
        "create",
        "a",
        &[id_of(&init)],
        None,
        json!({"key": "ex-1", "comments": [(comment.clone()), numbered]}),
    );
    let placed = merged(&comment, &json!({"where": null}));
    let refused = event(
        3,
        "create",
        "a",
        &[id_of(&kept)],
        None,
        json!({"key": "ex-2", "comments": [placed]}),
    );
    let mut journal = Journal::new();
    for event in [&init, &kept, &refused] {
        journal.add(event.clone(), "a", "", JournalOrder::Sequenced);
    }
    let state = replay(journal).unwrap();
    assert_eq!(
        state.issues().map(Issue::uid).collect::<Vec<_>>(),
        [uid(&kept)]
    );
    assert_eq!(problems(&state), ["create event has a malformed comment"]);
    let shown = Json::from(state.detail(state.issue(uid(&kept)).unwrap()));
    let shown: Vec<&Json> = shown["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["id"])
        .collect();
    assert_eq!(shown, [&Json::Null, &json!(7)]);
    let records = state.export_records();
    let exported: Vec<&Json> = records[0]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["id"])
        .collect();
    assert_eq!(exported, [&Json::Null, &json!(7)]);
}

/// U10: the stand-in for an id with several contents named the issue of the copy placed first; when no
/// copy was valid that could be a list or an object, and replay failed hashing it.
#[test]
fn u10_a_quarantined_id_names_its_issue_only_by_a_string() {
    let cases = [
        (json!([1]), None),
        (json!({"a": 1}), None),
        (json!(5), None),
        (json!("ex-1"), Some("ex-1")),
    ];
    for (issue, shown) in cases {
        let first = with(
            &event(1, "create", "a", &[], None, json!({})),
            "issue",
            issue.clone(),
        );
        let mut journal = Journal::new();
        for data in [json!({}), json!({"x": 1})] {
            journal.add(with(&first, "data", data), "a", "", JournalOrder::Sequenced);
        }
        let state = replay(journal).unwrap();
        let row = Integrity {
            event: Some(id_of(&first).to_owned()),
            issue: shown.map(str::to_owned),
            problem: "one event id with different contents".into(),
            origin: None,
        };
        assert_eq!(state.integrity(), [row], "{issue:?}");
    }
}

/// U09: an issue created with `created_at: null` (an import of such a record) made `list` and `ready`
/// fail comparing its time with the others'; a create cannot clear it, as it cannot clear a title.
#[test]
fn u09_a_create_cannot_clear_created_at() {
    let create = event(
        1,
        "create",
        "a",
        &[],
        None,
        json!({"key": "ex-1", "fields": {"created_at": null}}),
    );
    assert_eq!(
        payload_problem(&create).as_deref(),
        Some("field 'created_at' cannot be cleared")
    );
    for name in ["updated_at", "created_by", "closed_at"] {
        let mut fields = Object::new();
        fields.insert(name.into(), Json::Null);
        let create = event(
            1,
            "create",
            "a",
            &[],
            None,
            json!({"key": "ex-1", "fields": (Json::from(fields))}),
        );
        assert_eq!(payload_problem(&create), None, "{name}");
    }
}
