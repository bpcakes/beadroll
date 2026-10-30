//! Short fields shown from their registers (roll-lqjj.41, X16): what each shows where a create, a
//! later write and what replay used to copy out of a register into the issue differ, case by
//! case, in the state replay makes and in the one read back from a projection's state part.

use beadroll_core::event::Field;
use beadroll_core::op::IntegerField;
use beadroll_core::projection::{decode_state, project};
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_core::time::event_time;
use beadroll_test_support::events::{Event, id, id_at, journal};
use beadroll_test_support::{Json, json};

/// Whether a create imports an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CreateOrigin {
    /// Local.
    Local,
    /// Imported.
    Imported,
}

/// The issue every test is about: created by event 2.
fn uid() -> String {
    id(2)
}

/// An `init`, and a create of `fields`, on machine `a` (with `imported` when it is an import).
fn created(fields: Json, imported: CreateOrigin) -> Vec<Json> {
    let imported = matches!(imported, CreateOrigin::Imported);
    let mut data = json!({"key": "sf-a", "fields": fields});
    if imported && let Json::Object(members) = &mut data {
        members.insert("imported".into(), Json::Bool(true));
    }
    vec![
        Event::new(&id(1), "init")
            .data(json!({"prefix": "sf"}))
            .json(),
        Event::new(&uid(), "create")
            .deps(&[&id(1)])
            .data(data)
            .json(),
    ]
}

/// Event `n`, `op` on the issue by machine `replica`, built on `deps`.
fn on(n: u64, replica: &str, deps: &[u64], op: &str, data: Json) -> Json {
    let deps: Vec<String> = deps.iter().map(|n| id(*n)).collect();
    let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
    Event::new(&id(n), op)
        .replica(replica)
        .deps(&deps)
        .issue(&uid())
        .data(data)
        .json()
}

/// A `set` of `fields` as event `n` on machine `a`, built on event `n - 1`.
fn set(n: u64, fields: Json) -> Json {
    on(n, "a", &[n - 1], "set", json!({"fields": fields}))
}

/// The state replay makes of `events`, and the one a command that only reads decodes from their
/// projection: each field must show the same in both.
fn both(events: &[Json]) -> [State; 2] {
    let replayed = replay(journal(events)).unwrap();
    let (encoded, _) = project(journal(events)).unwrap().encode(0);
    [replayed, decode_state(&encoded.state).unwrap()]
}

/// What `field` of the issue shows, in both states alike.
fn shown(events: &[Json], field: Field) -> Option<Json> {
    let [replayed, decoded] = both(events);
    let of = |st: &State| st.issue_named(&uid()).unwrap().field(field).cloned();
    assert_eq!(of(&replayed), of(&decoded), "{}", field.name());
    of(&replayed)
}

/// The time of event `n`, as replay stamps it.
fn time(n: u64) -> Json {
    let id = beadroll_core::id::EventId::parse(&id(n)).unwrap();
    Json::from(event_time(id.millis()))
}

#[test]
fn a_creates_null_shows_null_and_a_later_writes_null_shows_nothing() {
    let mut events = created(
        json!({"title": "t", "assignee": null, "estimated_minutes": null, "external_ref": null}),
        CreateOrigin::Local,
    );
    for field in [Field::Assignee, Field::EstimatedMinutes, Field::ExternalRef] {
        assert_eq!(shown(&events, field), Some(Json::Null), "{}", field.name());
    }
    events.push(set(3, json!({"assignee": "ann", "estimated_minutes": 5})));
    assert_eq!(shown(&events, Field::Assignee), Some(json!("ann")));
    assert_eq!(shown(&events, Field::EstimatedMinutes), Some(json!(5)));
    assert_eq!(shown(&events, Field::ExternalRef), Some(Json::Null));
    events.push(set(
        4,
        json!({"assignee": null, "estimated_minutes": null, "external_ref": null}),
    ));
    for field in [Field::Assignee, Field::EstimatedMinutes, Field::ExternalRef] {
        assert_eq!(shown(&events, field), None, "{}", field.name());
    }
}

#[test]
fn a_creates_null_a_write_that_had_not_seen_it_leaves_largest_shows_nothing() {
    // The write is placed after the create by a cause that sorts after it, but has a smaller id
    // and never saw it: the register keeps both, the create's null the larger. A short field shows
    // nothing, as replay showed it once a write had touched the register; a long field the
    // create's null (roll-lqjj.10).
    let late = id_at(1_762_400_000_000, 9);
    let mut events = created(
        json!({"assignee": null, "notes": null}),
        CreateOrigin::Local,
    );
    events[1] = Event::new(&id_at(1_762_400_000_001, 2), "create")
        .deps(&[&id(1)])
        .data(json!({"key": "sf-a", "fields": {"assignee": null, "notes": null}}))
        .json();
    let uid = id_at(1_762_400_000_001, 2);
    events.push(
        Event::new(&id_at(1_762_400_000_002, 3), "comment")
            .replica("b")
            .deps(&[&id(1)])
            .issue(&uid)
            .data(json!({"text": "x"}))
            .json(),
    );
    let after = id_at(1_762_400_000_002, 3);
    events.push(
        Event::new(&late, "set")
            .replica("c")
            .deps(&[&after])
            .issue(&uid)
            .data(json!({"fields": {"assignee": "ann", "notes": "n"}}))
            .json(),
    );
    let [replayed, decoded] = both(&events);
    for st in [&replayed, &decoded] {
        let issue = st.issue_named(&uid).unwrap();
        assert_eq!(issue.registers().get(&Field::Assignee).unwrap().len(), 2);
        assert_eq!(issue.field(Field::Assignee), None);
    }
    let issue = replayed.issue_named(&uid).unwrap();
    assert_eq!(issue.field(Field::Notes), Some(&Json::Null));
}

#[test]
fn a_create_that_gives_nothing_shows_its_defaults_until_a_write() {
    let mut events = created(json!({}), CreateOrigin::Local);
    assert_eq!(shown(&events, Field::Title), Some(json!("")));
    assert_eq!(shown(&events, Field::Status), Some(json!("open")));
    assert_eq!(shown(&events, Field::Priority), Some(json!(2)));
    assert_eq!(shown(&events, Field::IssueType), Some(json!("task")));
    for field in [
        Field::Assignee,
        Field::EstimatedMinutes,
        Field::ExternalRef,
        Field::ClosedAt,
        Field::CloseReason,
        Field::Description,
    ] {
        assert_eq!(shown(&events, field), None, "{}", field.name());
    }
    assert_eq!(shown(&events, Field::CreatedAt), Some(time(2)));
    assert_eq!(shown(&events, Field::UpdatedAt), Some(time(2)));
    assert_eq!(shown(&events, Field::CreatedBy), Some(json!("test:a@a")));
    let [replayed, _] = both(&events);
    let issue = replayed.issue_named(&uid()).unwrap();
    assert_eq!(issue.priority(), IntegerField::from(2));
    // a default is no register: nothing is disputed, and a write gives the field its register
    assert_eq!(issue.registers().len(), 1);
    events.push(set(
        3,
        json!({"title": "named", "priority": 0, "issue_type": "bug"}),
    ));
    assert_eq!(shown(&events, Field::Title), Some(json!("named")));
    assert_eq!(shown(&events, Field::Priority), Some(json!(0)));
    assert_eq!(shown(&events, Field::IssueType), Some(json!("bug")));
    assert_eq!(shown(&events, Field::UpdatedAt), Some(time(3)));
    assert_eq!(shown(&events, Field::CreatedAt), Some(time(2)));
}

#[test]
fn an_import_shows_the_stamps_and_close_it_gives_and_an_unknown_author() {
    let record = json!({
        "title": "old",
        "status": "closed",
        "closed_at": "2026-01-03T00:00:00Z",
        "close_reason": "shipped",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z"
    });
    let mut events = created(record, CreateOrigin::Imported);
    assert_eq!(shown(&events, Field::Status), Some(json!("closed")));
    assert_eq!(
        shown(&events, Field::ClosedAt),
        Some(json!("2026-01-03T00:00:00Z"))
    );
    assert_eq!(shown(&events, Field::CloseReason), Some(json!("shipped")));
    assert_eq!(
        shown(&events, Field::CreatedAt),
        Some(json!("2026-01-01T00:00:00Z"))
    );
    assert_eq!(
        shown(&events, Field::UpdatedAt),
        Some(json!("2026-01-02T00:00:00Z"))
    );
    // the record did not say who made it: unknown, not the importer
    assert_eq!(shown(&events, Field::CreatedBy), Some(Json::Null));
    // an import that says null, or someone
    for (given, expected) in [(Json::Null, Json::Null), (json!("seed"), json!("seed"))] {
        let events = created(
            json!({"title": "t", "created_by": given}),
            CreateOrigin::Imported,
        );
        assert_eq!(shown(&events, Field::CreatedBy), Some(expected));
    }
    // a create made here that gives no author is its writer's
    let native = created(json!({"title": "t"}), CreateOrigin::Local);
    assert_eq!(shown(&native, Field::CreatedBy), Some(json!("test:a@a")));
    // a later event stamps the time it touched the issue; a reopen ends the close it gave
    events.push(on(3, "a", &[2], "reopen", json!({})));
    assert_eq!(shown(&events, Field::UpdatedAt), Some(time(3)));
    assert_eq!(shown(&events, Field::Status), Some(json!("open")));
    assert_eq!(shown(&events, Field::ClosedAt), None);
    assert_eq!(shown(&events, Field::CloseReason), None);
    assert_eq!(
        shown(&events, Field::CreatedAt),
        Some(json!("2026-01-01T00:00:00Z"))
    );
}

#[test]
fn a_close_a_reopen_and_a_close_again_show_the_status_registers_members() {
    let mut events = created(json!({"title": "t"}), CreateOrigin::Local);
    events.push(on(3, "a", &[2], "close", json!({"reason": "first"})));
    assert_eq!(shown(&events, Field::Status), Some(json!("closed")));
    assert_eq!(shown(&events, Field::ClosedAt), Some(time(3)));
    assert_eq!(shown(&events, Field::CloseReason), Some(json!("first")));
    events.push(on(4, "a", &[3], "reopen", json!({})));
    assert_eq!(shown(&events, Field::Status), Some(json!("open")));
    assert_eq!(shown(&events, Field::ClosedAt), None);
    assert_eq!(shown(&events, Field::CloseReason), None);
    events.push(on(5, "a", &[4], "close", json!({})));
    assert_eq!(shown(&events, Field::Status), Some(json!("closed")));
    assert_eq!(shown(&events, Field::ClosedAt), Some(time(5)));
    assert_eq!(shown(&events, Field::CloseReason), Some(json!("")));
    // a status a set writes is a member too, with the close it implies
    events.push(set(6, json!({"status": "deferred"})));
    assert_eq!(shown(&events, Field::ClosedAt), None);
    events.push(set(7, json!({"status": "tombstone"})));
    assert_eq!(shown(&events, Field::ClosedAt), Some(time(7)));
    assert_eq!(shown(&events, Field::CloseReason), Some(json!("")));
    let [replayed, _] = both(&events);
    let issue = replayed.issue_named(&uid()).unwrap();
    let (_, status) = issue
        .registers()
        .get(&Field::Status)
        .unwrap()
        .last()
        .unwrap();
    let members = json!({"status": "tombstone", "closed_at": (time(7)), "close_reason": ""});
    assert_eq!(*status, members);
}

#[test]
fn a_status_member_given_null_shows_null() {
    let events = created(
        json!({"title": "t", "status": "closed", "closed_at": null, "close_reason": null}),
        CreateOrigin::Imported,
    );
    assert_eq!(shown(&events, Field::Status), Some(json!("closed")));
    assert_eq!(shown(&events, Field::ClosedAt), Some(Json::Null));
    assert_eq!(shown(&events, Field::CloseReason), Some(Json::Null));
}

#[test]
fn a_set_of_closed_at_or_close_reason_shows_until_a_status_is_written_after_it() {
    let mut events = created(json!({"title": "t"}), CreateOrigin::Local);
    events.push(on(3, "a", &[2], "close", json!({"reason": "done"})));
    events.push(set(4, json!({"closed_at": "2026-05-05T00:00:00Z"})));
    assert_eq!(
        shown(&events, Field::ClosedAt),
        Some(json!("2026-05-05T00:00:00Z"))
    );
    assert_eq!(shown(&events, Field::CloseReason), Some(json!("done")));
    events.push(set(5, json!({"close_reason": null})));
    assert_eq!(shown(&events, Field::CloseReason), None);
    assert_eq!(
        shown(&events, Field::ClosedAt),
        Some(json!("2026-05-05T00:00:00Z"))
    );
    // a status written after them shows the status register's members again
    events.push(on(6, "a", &[5], "reopen", json!({})));
    assert_eq!(shown(&events, Field::ClosedAt), None);
    assert_eq!(shown(&events, Field::CloseReason), None);
    // within one set, the later member of the two wins
    let mut first = events.clone();
    first.push(set(
        7,
        json!({"closed_at": "2026-06-06T00:00:00Z", "status": "closed"}),
    ));
    assert_eq!(shown(&first, Field::ClosedAt), Some(time(7)));
    let mut after = events;
    after.push(set(
        7,
        json!({"status": "closed", "closed_at": "2026-06-06T00:00:00Z"}),
    ));
    assert_eq!(
        shown(&after, Field::ClosedAt),
        Some(json!("2026-06-06T00:00:00Z"))
    );
    assert_eq!(shown(&after, Field::CloseReason), Some(json!("")));
}

#[test]
fn a_register_concurrent_writes_left_with_several_entries_shows_the_largest_id() {
    let mut events = created(
        json!({"title": "t", "estimated_minutes": 10}),
        CreateOrigin::Local,
    );
    // machines b and c each write without seeing the other; c's event has the larger id
    for (n, replica) in [(3_u64, "b"), (4, "c")] {
        let value = format!("from {replica}");
        let number = i64::try_from(n).unwrap();
        let fields = json!({"title": (value.as_str()), "assignee": (value.as_str()), "priority": (number - 2), "estimated_minutes": (number * 10), "external_ref": (value.as_str())});
        events.push(on(n, replica, &[2], "set", json!({"fields": fields})));
    }
    events.push(on(5, "b", &[3], "close", json!({"reason": "b"})));
    events.push(on(
        6,
        "c",
        &[4],
        "set",
        json!({"fields": {"status": "blocked"}}),
    ));
    let [replayed, decoded] = both(&events);
    for st in [&replayed, &decoded] {
        let issue = st.issue_named(&uid()).unwrap();
        for field in [
            Field::Title,
            Field::Assignee,
            Field::Priority,
            Field::EstimatedMinutes,
            Field::ExternalRef,
            Field::Status,
        ] {
            assert_eq!(
                issue.registers().get(&field).unwrap().len(),
                2,
                "{}",
                field.name()
            );
        }
        assert_eq!(issue.title(), "from c");
        assert_eq!(issue.field(Field::Assignee), Some(&json!("from c")));
        assert_eq!(issue.priority(), IntegerField::from(2));
        assert_eq!(issue.field(Field::EstimatedMinutes), Some(&json!(40)));
        assert_eq!(issue.field(Field::ExternalRef), Some(&json!("from c")));
        assert_eq!(issue.status(), "blocked");
        assert_eq!(issue.field(Field::ClosedAt), None);
        assert_eq!(st.conflicts(issue).len(), 6);
    }
}

#[test]
fn a_legacy_boolean_priority_shows_as_written() {
    let mut events = created(
        json!({"title": "t", "priority": true}),
        CreateOrigin::Imported,
    );
    assert_eq!(shown(&events, Field::Priority), Some(Json::Bool(true)));
    let [replayed, decoded] = both(&events);
    for st in [&replayed, &decoded] {
        let issue = st.issue_named(&uid()).unwrap();
        assert_eq!(issue.priority(), IntegerField::Bool(true));
        // shown as the number it counts as (until roll-lqjj.53.2, as Python's `True`)
        assert_eq!(issue.priority().to_string(), "1");
    }
    events.push(set(3, json!({"priority": false})));
    assert_eq!(shown(&events, Field::Priority), Some(Json::Bool(false)));
}

#[test]
fn an_estimate_and_a_reference_show_what_was_given_set_and_cleared() {
    let mut events = created(
        json!({"title": "t", "estimated_minutes": 0, "external_ref": "gh-1"}),
        CreateOrigin::Local,
    );
    assert_eq!(shown(&events, Field::EstimatedMinutes), Some(json!(0)));
    assert_eq!(shown(&events, Field::ExternalRef), Some(json!("gh-1")));
    events.push(set(
        3,
        json!({"estimated_minutes": 90, "external_ref": "gh-2"}),
    ));
    assert_eq!(shown(&events, Field::EstimatedMinutes), Some(json!(90)));
    assert_eq!(shown(&events, Field::ExternalRef), Some(json!("gh-2")));
    events.push(set(4, json!({"estimated_minutes": null})));
    assert_eq!(shown(&events, Field::EstimatedMinutes), None);
    assert_eq!(shown(&events, Field::ExternalRef), Some(json!("gh-2")));
    // never given: absent, and nothing for a reader to dispute
    let bare = created(json!({"title": "t"}), CreateOrigin::Local);
    assert_eq!(shown(&bare, Field::EstimatedMinutes), None);
    assert_eq!(shown(&bare, Field::ExternalRef), None);
}
