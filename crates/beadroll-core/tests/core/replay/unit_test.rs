//! Unusable payloads are scoped and reported; unknown event formats change nothing. Remote URL
//! and coordination-ref checks live in git and store tests.

use beadroll_core::event::Field;
use beadroll_core::replay::replay;
use beadroll_test_support::events::{id, journal};
use beadroll_test_support::{Json, json};

/// One event: its operation, machine, causes (by position), payload, and issue (by position, or
/// the event itself).
type Spec<'a> = (&'a str, &'a str, &'a [usize], Json, Option<usize>);

/// Events built from the specs, with monotonically ordered ids a few milliseconds apart, and
/// their ids.
fn events(specs: Vec<Spec<'_>>) -> (Vec<Json>, Vec<String>) {
    let mut events = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for (op, machine, deps, data, issue) in specs {
        let event_id = id(u64::try_from(ids.len()).unwrap() + 1);
        let deps: Vec<Json> = deps.iter().map(|d| Json::from(ids[*d].as_str())).collect();
        let issue = issue.map_or_else(|| event_id.clone(), |at| ids[at].clone());
        events.push(json!({
            "v": 1,
            "id": (event_id.as_str()),
            "replica": machine,
            "deps": (Json::Array(deps)),
            "actor": {"agent": "codex", "thread": (format!("{machine}-t"))},
            "op": op,
            "data": data,
            "issue": (issue.as_str())
        }));
        ids.push(event_id);
    }
    (events, ids)
}

#[test]
fn malformed_payloads_are_scoped_and_reported() {
    let (mut all, ids) = events(vec![
        ("init", "a", &[], json!({"prefix": "ex"}), None),
        (
            "create",
            "a",
            &[0],
            json!({"key": "ex-ok", "fields": {"title": "fine"}}),
            None,
        ),
        (
            "create",
            "a",
            &[1],
            json!({"fields": {"title": "no key"}}),
            None,
        ), // missing key
        ("dep_add", "a", &[2], json!({"type": "blocks"}), Some(1)), // missing target
        ("comment", "a", &[3], json!({"text": "x"}), Some(1)),
    ]);
    all.push(json!({
        "v": 1,
        "id": (id(6).as_str()),
        "replica": "a",
        "deps": [(ids[4].as_str())],
        "actor": ["list"],
        "op": "set",
        "data": {"fields": {"title": "bad actor"}},
        "issue": (ids[1].as_str())
    }));
    let st = replay(journal(&all)).unwrap();
    let issue = st.issue_named(&ids[1]).unwrap();
    assert_eq!(issue.field(Field::Title), Some(&Json::from("fine")));
    assert_eq!(issue.comments().len(), 1);
    let mut problems: Vec<String> = st
        .integrity()
        .iter()
        .map(|row| row.problem.chars().take(6).collect())
        .collect();
    problems.sort();
    assert_eq!(problems, ["actor ", "create", "dep_ad"]);
}

#[test]
fn an_ignored_newer_format_event_cannot_change_ownership() {
    // A takeover encoded in a format this bead does not know.
    let claim = id(3);
    let (mut all, ids) = events(vec![
        ("init", "a", &[], json!({"prefix": "ex"}), None),
        (
            "create",
            "a",
            &[0],
            json!({"key": "ex-1", "fields": {"title": "t"}}),
            None,
        ),
        (
            "claim",
            "a",
            &[1],
            json!({"mode": "online", "generation": 1}),
            Some(1),
        ),
        (
            "claim",
            "b",
            &[2],
            json!({"mode": "online", "generation": 2, "takeover": (claim.as_str()), "reason": "future"}),
            Some(1),
        ),
        (
            "close",
            "a",
            &[2],
            json!({"claim": (claim.as_str()), "reason": "done"}),
            Some(1),
        ),
    ]);
    if let Json::Object(takeover) = &mut all[3] {
        takeover.insert("v".into(), Json::from(2));
    }
    let st = replay(journal(&all)).unwrap();
    let issue = st.issue_named(&ids[1]).unwrap();
    assert_eq!(issue.status(), "closed"); // the owner's own close stands
    assert!(issue.claim().is_none());
    assert!(issue.stale().is_empty());
    let problems: Vec<String> = st
        .integrity()
        .iter()
        .map(|row| row.problem.chars().take(12).collect())
        .collect();
    assert_eq!(problems, ["event format"]);
}
