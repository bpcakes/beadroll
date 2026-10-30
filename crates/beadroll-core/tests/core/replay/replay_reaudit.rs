//! Storage damage stays scoped to the offending event, and replayed state does not depend on
//! journal delivery order. The merged-parent-cycle command test is in the CLI suite
//! (`docs/FINDINGS.md`, REAUDIT).

mod scale;

use beadroll_core::event::{Field, FieldType, REGISTERS, REQUIRED_FIELDS};
use beadroll_core::journal::Journal;
use beadroll_core::journal::JournalOrder;
use beadroll_core::json::{Format, Int, write};
use beadroll_core::replay::replay;
use beadroll_core::state::{Integrity, State};
use beadroll_test_support::Json;
use beadroll_test_support::events::{Event, journal};
use beadroll_test_support::json;

/// Events with fixed UUIDv7s, so every order assertion is independent of the clock.
pub(crate) struct Fixture {
    serial: u64,
}

impl Fixture {
    pub(crate) fn new() -> Fixture {
        Fixture { serial: 0 }
    }

    /// The next event: `op` by machine `machine`, on `issue` (itself when `None`).
    pub(crate) fn event(
        &mut self,
        op: &str,
        machine: &str,
        deps: &[&str],
        issue: Option<&str>,
        data: Json,
    ) -> Json {
        self.serial += 1;
        let eid = format!("019a1111-1111-7000-8000-{:012x}", self.serial);
        Event::new(&eid, op)
            .replica(machine)
            .deps(deps)
            .issue(issue.unwrap_or(&eid))
            .actor(json!({"agent": "codex", "thread": (format!("{machine}-thread"))}))
            .data(data)
            .json()
    }

    /// The tracker's init, and one issue, `ex-ok`, titled `survivor`.
    pub(crate) fn fixture(&mut self) -> (Json, Json) {
        let init = self.event("init", "a", &[], None, json!({"prefix": "ex"}));
        let create = self.event(
            "create",
            "a",
            &[&id(&init)],
            None,
            json!({"key": "ex-ok", "fields": {"title": "survivor"}}),
        );
        (init, create)
    }
}

/// An event's id.
pub(crate) fn id(event: &Json) -> String {
    event["id"].as_str().unwrap().to_owned()
}

/// The state `events` replay to, each in its writer's journal.
fn replayed(events: &[Json]) -> State {
    replay(journal(events)).unwrap()
}

/// An integrity row as a JSON object.
pub(crate) fn row(row: &Integrity) -> Json {
    let mut members = json!({
        "event": (row.event.clone()),
        "issue": (row.issue.clone()),
        "problem": (row.problem.as_str())
    });
    if let (Json::Object(members), Some(origin)) = (&mut members, &row.origin) {
        members.insert("origin".into(), Json::from(origin.as_str()));
    }
    members
}

/// A deterministic text snapshot of export, issue details, integrity rows and pending events.
pub(crate) fn projection(journal: Journal) -> String {
    let st = replay(journal).unwrap();
    let canonical = |value: &Json| write(value, Format::CANONICAL).unwrap();
    let records = Json::Array(st.export_records().into_iter().map(Json::from).collect());
    let details = Json::Array(
        st.issues()
            .map(|issue| Json::from(st.detail(issue)))
            .collect(),
    );
    let mut integrity: Vec<Json> = st.integrity().iter().map(row).collect();
    integrity.sort_by_key(|row| canonical(row));
    let mut pending: Vec<String> = st.pending().iter().map(ToString::to_string).collect();
    pending.sort();
    canonical(&json!({
        "records": records,
        "details": details,
        "integrity": (Json::Array(integrity)),
        "pending": (Json::Array(pending.into_iter().map(Json::from).collect()))
    }))
}

/// The issue `event` created.
fn issue<'a>(st: &'a State, event: &Json) -> &'a beadroll_core::state::Issue {
    st.issue_named(&id(event)).unwrap()
}

/// A copy of `event` with member `name` set to `value`.
fn with(event: &Json, name: &str, value: Json) -> Json {
    let mut copy = event.clone();
    if let Json::Object(members) = &mut copy {
        members.insert(name.into(), value);
    }
    copy
}

#[test]
fn malformed_duplicate_order_does_not_change_projection() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let malformed = with(&create, "op", json!(["create"]));
    let build = |variant_first: bool| {
        let mut j = Journal::new();
        j.add(init.clone(), "a", "", JournalOrder::Sequenced);
        // Corruption in another machine's copy; valid envelope cannot be checked for placement.
        let mut rows = vec![(malformed.clone(), "b"), (create.clone(), "a")];
        if !variant_first {
            rows.reverse();
        }
        for (event, machine) in rows {
            j.add(event, machine, "", JournalOrder::Sequenced);
        }
        j
    };
    let (left, right) = (replay(build(true)).unwrap(), replay(build(false)).unwrap());
    assert_eq!(left.export_records(), right.export_records());
    assert!(
        left.issue_named(&id(&create)).is_none(),
        "ambiguous event must be ignored in either order"
    );
}

#[test]
fn quarantined_variant_cannot_hold_descendants_in_cycle() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let child = f.event(
        "create",
        "a",
        &[&id(&create)],
        None,
        json!({"key": "ex-child", "fields": {"title": "unrelated survivor"}}),
    );
    let variant = with(&create, "deps", json!([(id(&child))]));
    let st = replayed(&[init, create, child.clone(), variant]);
    assert!(
        st.pending().is_empty(),
        "quarantined event must not hold later frontier events"
    );
    assert!(st.issue_named(&id(&child)).is_some());
    assert_eq!(st.integrity().len(), 1);
}

#[test]
fn invalid_timestamp_is_scoped_even_when_event_is_inert() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let invalid = f.event("future_op", "b", &[&id(&create)], None, json!({}));
    let invalid = with(
        &invalid,
        "id",
        json!("ffffffff-ffff-7000-8000-000000000000"),
    );
    let st = replayed(&[init, create.clone(), invalid]);
    assert_eq!(issue(&st, &create).title(), "survivor");
    assert!(!st.integrity().is_empty());
}

#[test]
fn non_scalar_actor_thread_is_scoped() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let invalid = f.event(
        "comment",
        "b",
        &[&id(&create)],
        Some(&id(&create)),
        json!({"text": "bad"}),
    );
    let invalid = with(
        &invalid,
        "actor",
        json!({"agent": "codex", "thread": ["not", "scalar"]}),
    );
    let st = replayed(&[init, create.clone(), invalid]);
    assert!(issue(&st, &create).comments().is_empty());
    assert!(!st.integrity().is_empty());
}

#[test]
fn malformed_set_is_atomic_and_cannot_overwrite_internal_state() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let invalid = f.event(
        "set",
        "b",
        &[&id(&create)],
        Some(&id(&create)),
        json!({"fields": {"title": "partial poison", "reg": null}}),
    );
    let st = replayed(&[init, create.clone(), invalid]);
    let survivor = issue(&st, &create);
    assert_eq!(survivor.title(), "survivor");
    // The create's registers remain unchanged.
    assert_eq!(
        survivor
            .registers()
            .get(&Field::Title)
            .map(|register| register.len()),
        Some(1)
    );
    assert!(!st.integrity().is_empty());
}

#[test]
fn duplicate_create_uid_does_not_destroy_issue() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let overwrite = f.event(
        "create",
        "b",
        &[&id(&create)],
        Some(&id(&create)),
        json!({"key": "ex-other", "fields": {"title": "replacement"}}),
    );
    let st = replayed(&[init, create, overwrite]);
    assert_eq!(st.resolve("ex-ok").unwrap().title(), "survivor");
    assert!(!st.integrity().is_empty());
}

#[test]
fn offline_forged_takeover_stays_provisional_and_does_not_fence_close() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let uid = id(&create);
    let claim = f.event(
        "claim",
        "a",
        &[&uid],
        Some(&uid),
        json!({"mode": "online", "generation": 1}),
    );
    let fake = f.event(
        "claim",
        "b",
        &[&id(&claim)],
        Some(&uid),
        json!({"mode": "offline", "generation": 999, "takeover": (id(&claim)), "reason": "offline"}),
    );
    let close = f.event(
        "close",
        "a",
        &[&id(&claim)],
        Some(&uid),
        json!({"claim": (id(&claim))}),
    );
    let st = replayed(&[init, create.clone(), claim, fake, close]);
    let issue = issue(&st, &create);
    assert_eq!(issue.status(), "closed");
    assert!(issue.claim().is_none());
    assert!(issue.stale().is_empty());
    assert_eq!(*issue.claim_generation(), Int::from(1));
}

#[test]
fn orphan_takeover_does_not_fence_another_issues_close() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let uid = id(&create);
    let claim = f.event(
        "claim",
        "a",
        &[&uid],
        Some(&uid),
        json!({"mode": "online", "generation": 1}),
    );
    let orphan = f.event(
        "claim",
        "b",
        &[&id(&claim)],
        Some("019a1111-1111-7000-8000-111111111111"),
        json!({"mode": "online", "generation": 2, "takeover": (id(&claim)), "reason": "wrong target"}),
    );
    let close = f.event(
        "close",
        "a",
        &[&id(&claim)],
        Some(&uid),
        json!({"claim": (id(&claim))}),
    );
    let st = replayed(&[init, create.clone(), claim, orphan, close]);
    let issue = issue(&st, &create);
    assert_eq!(issue.status(), "closed");
    assert!(issue.claim().is_none());
    assert!(issue.stale().is_empty());
}

#[test]
fn partially_invalid_label_add_applies_nothing() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let uid = id(&create);
    let bad = f.event(
        "label_add",
        "b",
        &[&uid],
        Some(&uid),
        json!({"labels": ["must-not-stick", {}]}),
    );
    let st = replayed(&[init, create.clone(), bad]);
    assert!(!st.integrity().is_empty());
    assert!(
        issue(&st, &create).labels().is_empty(),
        "event reported as invalid must not mutate valid state"
    );
}

#[test]
fn all_registers_preserve_none_against_concurrent_value() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let uid = id(&create);
    // Q09: the required registers cannot be cleared.
    let clearable: Vec<Field> = REGISTERS
        .into_iter()
        .filter(|field| !REQUIRED_FIELDS.contains(field))
        .collect();
    let fields = |value: &dyn Fn(Field) -> Json| {
        let members = clearable
            .iter()
            .map(|field| (field.name().to_owned(), value(*field)));
        json!({"fields": (Json::Object(Box::new(members.collect())))})
    };
    let left = f.event("set", "b", &[&uid], Some(&uid), fields(&|_| Json::Null));
    let a_value = |field: Field| {
        if field.kind() == FieldType::Int {
            json!(1)
        } else {
            json!("a value")
        }
    };
    let right = f.event("set", "c", &[&uid], Some(&uid), fields(&a_value));
    let mut j = journal(&[init, create.clone(), left.clone(), right.clone()]);
    let st = replay(j.clone()).unwrap();
    let mut disputed: Vec<Field> = st
        .conflicts(issue(&st, &create))
        .into_iter()
        .map(|(field, _)| field)
        .collect();
    disputed.sort();
    assert_eq!(disputed, clearable);
    let settled = f.event(
        "set",
        "d",
        &[&id(&left), &id(&right)],
        Some(&uid),
        fields(&|_| Json::Null),
    );
    j.add(settled, "d", "", JournalOrder::Sequenced);
    let st = replay(j).unwrap();
    assert!(st.conflicts(issue(&st, &create)).is_empty());
}

#[test]
fn missing_predecessor_holds_claim_and_close_then_recovers() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let uid = id(&create);
    let claim = f.event(
        "claim",
        "b",
        &[&uid],
        Some(&uid),
        json!({"mode": "online", "generation": 1}),
    );
    let close = f.event(
        "close",
        "b",
        &[&id(&claim)],
        Some(&uid),
        json!({"claim": (id(&claim))}),
    );
    let mut j = journal(&[init, claim.clone(), close.clone()]);
    let st = replay(j.clone()).unwrap();
    let mut pending: Vec<String> = st.pending().iter().map(ToString::to_string).collect();
    pending.sort();
    let mut expected = vec![id(&claim), id(&close)];
    expected.sort();
    assert_eq!(pending, expected);
    assert_eq!(st.issues().count(), 0);
    j.add(create.clone(), "a", "", JournalOrder::Sequenced);
    let st = replay(j).unwrap();
    assert!(st.pending().is_empty());
    assert_eq!(issue(&st, &create).status(), "closed");
    assert!(issue(&st, &create).claim().is_none());
}

#[test]
fn ignored_op_with_valid_id_does_not_hold_descendant() {
    let mut f = Fixture::new();
    let (init, create) = f.fixture();
    let unknown = f.event("future_op", "a", &[&id(&create)], None, json!({}));
    let child = f.event(
        "comment",
        "a",
        &[&id(&unknown)],
        Some(&id(&create)),
        json!({"text": "survivor"}),
    );
    let st = replayed(&[init, create.clone(), unknown, child]);
    assert!(st.pending().is_empty());
    assert_eq!(st.integrity().len(), 1);
    let texts: Vec<Json> = issue(&st, &create)
        .comments()
        .iter()
        .filter_map(|c| c.get("text"))
        .collect();
    assert_eq!(texts, vec![json!("survivor")]);
}
