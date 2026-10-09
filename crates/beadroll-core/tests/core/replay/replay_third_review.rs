//! `ReplayThirdReview`: the third independent review (REVIEW3, 2026-10-06; findings in
//! `docs/FINDINGS.md`). A quarantined id stays scoped to itself, malformed embedded comments and
//! creates without an issue are inert, exported comment ids survive later arrivals, register clears
//! stay queryable, a cycle left after a partial removal is reported, and the ancestry of mixed
//! sequenced and attempt-file events is exact.

use std::collections::{BTreeSet, HashMap};

use beadroll_core::id::EventId;
use beadroll_core::journal::Journal;
use beadroll_core::json::Value;
use beadroll_core::query::Edges;
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_test_support::events::Event;
use beadroll_test_support::{Json, json};
use sha2::{Digest, Sha256};

mod ancestry;

/// The class's events: ids `019a1111-1111-7000-8000-<serial>`, numbered as they are made.
struct Fixture {
    serial: u64,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture { serial: 0 }
    }

    fn next_id(&mut self) -> String {
        self.serial += 1;
        format!("019a1111-1111-7000-8000-{:012x}", self.serial)
    }

    /// An event about `issue` (itself when `None`), written by `replica`, naming `deps`.
    fn event(
        &mut self,
        op: &str,
        replica: &str,
        deps: &[&str],
        issue: Option<&str>,
        data: Json,
    ) -> Json {
        let id = self.next_id();
        built(&id, op, replica, deps, issue, data)
    }

    /// `init`, and the create of `ex-one`, titled `survivor`.
    fn base(&mut self) -> (Json, Json) {
        let init = self.event("init", "a", &[], None, json!({"prefix": "ex"}));
        let data = json!({"key": "ex-one", "fields": {"title": "survivor"}});
        let create = self.event("create", "a", &[id_of(&init)], None, data);
        (init, create)
    }
}

/// Event `id` as the fixture writes it.
fn built(
    id: &str,
    op: &str,
    replica: &str,
    deps: &[&str],
    issue: Option<&str>,
    data: Json,
) -> Json {
    let event = Event::new(id, op).replica(replica).deps(deps).data(data);
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

/// `event` with the member at `path` set to `value`.
fn with(event: &Json, path: &[&str], value: Json) -> Json {
    let mut event = event.clone();
    let mut at = &mut event;
    for name in &path[..path.len() - 1] {
        let Json::Object(members) = at else {
            panic!("no object at {name}")
        };
        at = members.get_mut(*name).unwrap();
    }
    let Json::Object(members) = at else {
        panic!("no object to set {path:?} in")
    };
    members.insert(path[path.len() - 1].to_owned(), value);
    event
}

/// A settled journal of `events`, each in its writer's journal; the `detached` ones unsequenced, as
/// an attempt file's events are.
fn journal(events: &[Json], detached: &BTreeSet<String>) -> Journal {
    let mut journal = Journal::new();
    for event in events {
        let replica = event["replica"].as_str().unwrap().to_owned();
        let sequenced = !detached.contains(id_of(event));
        journal.add(event.clone(), &replica, "", sequenced);
    }
    journal.settle();
    journal
}

fn replayed(events: &[Json]) -> State {
    replay(journal(events, &BTreeSet::new())).unwrap()
}

/// The texts of an issue's comments, in order.
fn texts(state: &State, create: &Json) -> Vec<String> {
    let issue = state.issue(uid(create)).unwrap();
    issue
        .comments()
        .iter()
        .map(|c| c.get("text").unwrap().as_str().unwrap().to_owned())
        .collect()
}

/// The exported id of the comment that says `text`, on the record `key`.
fn exported_comment(state: &State, key: &str, text: &str) -> Value {
    let records = state.export_records();
    let record = records
        .iter()
        .find(|r| r["id"].as_str() == Some(key))
        .unwrap();
    let comments = record["comments"].as_array().unwrap();
    comments
        .iter()
        .find(|c| c["text"].as_str() == Some(text))
        .unwrap()["id"]
        .clone()
}

/// A quarantined comment whose variants agree on a cause that loops back to it is released, applies
/// nothing, and does not hold the comment built on it.
#[test]
fn quarantine_agreed_cycle_is_scoped() {
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let bad_id = f.next_id();
    let child = f.event(
        "comment",
        "c",
        &[&bad_id],
        Some(id_of(&create)),
        json!({"text": "surviving child"}),
    );
    let bad = built(
        &bad_id,
        "comment",
        "b",
        &[id_of(&child)],
        Some(id_of(&create)),
        json!({"text": "variant one"}),
    );
    let variant = with(&bad, &["data", "text"], json!("variant two"));
    let state = replay(journal(
        &[init, create.clone(), bad, child, variant],
        &BTreeSet::new(),
    ))
    .unwrap();
    assert!(state.pending().is_empty());
    assert_eq!(texts(&state, &create), ["surviving child"]);
    assert!(!state.integrity().is_empty());
}

/// An id whose second copy has a scalar for its causes is quarantined, applies nothing, and replay
/// goes on.
#[test]
fn quarantine_malformed_scalar_deps_is_scoped() {
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let comment = f.event(
        "comment",
        "b",
        &[id_of(&create)],
        Some(id_of(&create)),
        json!({"text": "ambiguous"}),
    );
    let variant = with(&comment, &["deps"], json!(17));
    let state = replayed(&[init, create.clone(), comment, variant]);
    assert!(texts(&state, &create).is_empty());
    assert!(!state.integrity().is_empty());
}

/// An embedded comment without its author and time makes its create inert, and every query and the
/// export keep working.
#[test]
fn malformed_imported_comment_does_not_break_queries() {
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let create = with(
        &create,
        &["data", "comments"],
        json!([{"text": "missing author and timestamp"}]),
    );
    let state = replayed(&[init, create.clone()]);
    state.export_records();
    for issue in state.issues() {
        state.detail(issue);
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
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let Json::Object(mut members) = create else {
        panic!("an event is an object")
    };
    members.shift_remove("issue");
    let state = replayed(&[init, Json::Object(members)]);
    assert!(
        !state.integrity().is_empty(),
        "missing create issue ID must not install inaccessible None-keyed issue"
    );
    assert_eq!(state.issues().count(), 0);
}

/// A legacy comment imported later with the id a comment made here already exports does not renumber
/// that comment.
#[test]
fn new_comment_integer_survives_collision_with_later_legacy_import() {
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let comment = f.event(
        "comment",
        "a",
        &[id_of(&create)],
        Some(id_of(&create)),
        json!({"text": "existing"}),
    );
    let events = vec![init, create, comment.clone()];
    let before = replayed(&events).export_records()[0]["comments"][0]["id"].clone();
    let legacy = json!({"legacy_id": (before.clone()), "author": "legacy", "text": "legacy", "created_at": "2020-01-01T00:00:00Z"});
    let data = json!({"key": "ex-two", "fields": {"title": "imported"}, "comments": [legacy], "imported": true});
    let other = f.event("create", "a", &[id_of(&comment)], None, data);
    let after = replayed(&[events, vec![other]].concat());
    let records = after.export_records();
    let record = records
        .iter()
        .find(|r| r["id"].as_str() == Some("ex-one"))
        .unwrap();
    let existing = &record["comments"][0]["id"];
    assert_eq!(
        *existing, before,
        "later imported legacy ID renumbers an already-exported bead comment"
    );
}

/// An identical copy of a machine's event in another machine's journal, read before or after the
/// real one, does not reorder or break the real journal.
#[test]
fn identical_misplaced_duplicate_does_not_reorder_own_journal() {
    let mut f = Fixture::new();
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
        journal.add(init.clone(), "a", "", true);
        journal.add(create.clone(), "a", "", true);
        if misplaced_first {
            journal.add(second.clone(), "b", "", true);
        }
        journal.add(first.clone(), "z", "", true);
        journal.add(second.clone(), "z", "", true);
        if !misplaced_first {
            journal.add(second.clone(), "b", "", true);
        }
        journal.settle();
        let state = replay(journal).unwrap();
        results.push((texts(&state, &create), state.chain_breaks().clone()));
    }
    assert_eq!(
        results[0], results[1],
        "misplaced identical duplicate reordered the actual owner journal"
    );
    assert_eq!(results[0].0, ["first", "second"]);
    assert!(results[0].1.is_empty());
}

/// Two valid comments whose ids collided under the retired seven-digit formula: the comment
/// exported first keeps its id when the other arrives from another machine.
#[test]
fn valid_colliding_comments_keep_exported_ids_when_other_machine_arrives() {
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let mut seen: HashMap<u64, String> = HashMap::new();
    let mut pair = None;
    for n in 0..100_000u64 {
        let id = format!("019a1111-2222-7000-8000-{n:012x}");
        let digest = Sha256::digest(id.as_bytes());
        let head = u64::from(u32::from_be_bytes([
            digest[0], digest[1], digest[2], digest[3],
        ]));
        let number = 1_000_000 + head % 9_000_000;
        if let Some(earlier) = seen.get(&number) {
            pair = Some((earlier.clone(), id));
            break;
        }
        seen.insert(number, id);
    }
    let (earlier_id, later_id) = pair.expect("deterministic fixture should find a hash collision");
    let on = Some(id_of(&create));
    let earlier = built(
        &earlier_id,
        "comment",
        "b",
        &[id_of(&create)],
        on,
        json!({"text": "arrives later"}),
    );
    let later = built(
        &later_id,
        "comment",
        "c",
        &[id_of(&create)],
        on,
        json!({"text": "already exported"}),
    );
    let state = replayed(&[init.clone(), create.clone(), later.clone()]);
    let before = state.export_records()[0]["comments"][0]["id"].clone();
    let state = replayed(&[init, create, later, earlier]);
    assert!(state.integrity().is_empty());
    let after = exported_comment(&state, "ex-one", "already exported");
    assert_eq!(
        after, before,
        "valid comments ({earlier_id}, {later_id}) collide and remote arrival renumbers existing export"
    );
}

/// Clearing optional registers is valid, and leaves every query and the export working.
#[test]
fn optional_register_clears_remain_queryable() {
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let fields = json!({"description": null, "design": null, "notes": null, "assignee": null});
    let clear = f.event(
        "set",
        "a",
        &[id_of(&create)],
        Some(id_of(&create)),
        json!({"fields": fields}),
    );
    let state = replayed(&[init, create.clone(), clear]);
    assert!(state.integrity().is_empty());
    state.export_records();
    let issue = state.issue(uid(&create)).unwrap();
    state.detail(issue);
    assert_eq!(issue.title(), "survivor");
}

/// Two elementary cycles share an edge; removing a leaf edge of one leaves the other reported, and
/// every issue on it blocked.
#[test]
fn merged_dependency_cycle_reports_remaining_cycle_after_partial_removal() {
    let mut f = Fixture::new();
    let (init, first) = f.base();
    let second = f.event(
        "create",
        "a",
        &[id_of(&first)],
        None,
        json!({"key": "ex-two", "fields": {"title": "two"}}),
    );
    let third = f.event(
        "create",
        "a",
        &[id_of(&second)],
        None,
        json!({"key": "ex-three", "fields": {"title": "three"}}),
    );
    let mut events = vec![init, first.clone(), second.clone(), third.clone()];
    let mut previous = id_of(&third).to_owned();
    for (source, target) in [
        (&first, &second),
        (&second, &first),
        (&second, &third),
        (&third, &first),
    ] {
        let data = json!({"to": (id_of(target)), "type": "blocks"});
        let event = f.event("dep_add", "a", &[&previous], Some(id_of(source)), data);
        previous = id_of(&event).to_owned();
        events.push(event);
    }
    let before = replayed(&events);
    assert_eq!(before.cycles(Edges::WaitsFor).len(), 2);
    let data = json!({"to": (id_of(&first)), "type": "blocks"});
    let removed = f.event("dep_remove", "a", &[&previous], Some(id_of(&second)), data);
    let after = replayed(&[events, vec![removed]].concat());
    assert_eq!(
        after.cycles(Edges::WaitsFor),
        [["ex-one", "ex-two", "ex-three"]]
    );
    assert!(after.issues().all(|issue| after.is_blocked(issue)));
}
