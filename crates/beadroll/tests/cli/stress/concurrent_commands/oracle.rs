//! What a wave is checked against besides convergence: the claims the receipts lead to, blockers
//! and readiness worked out from the details alone, and each machine settled.

use std::collections::{BTreeMap, BTreeSet};

use beadroll_test_support::{Json, Machine, parse_json};

use crate::stress::{id, json_file, status_disputed};

/// The ids of the issues `export` holds, sorted.
pub(super) fn exported_keys(export: &str) -> Vec<String> {
    let mut keys: Vec<String> = (export.lines())
        .map(|line| id(&parse_json(line).expect("an exported record")))
        .collect();
    keys.sort();
    keys
}

/// Each owned issue in `details` has the claim the receipts lead to, or none.
pub(super) fn check_claims(
    wave: usize,
    details: &Json,
    expected: &BTreeMap<String, Option<String>>,
) {
    for issue in details.as_array().expect("bead show prints a list") {
        if let Some(want) = expected.get(&id(issue)) {
            let held = issue.get("claim").filter(|claim| claim.truthy()).map(id);
            assert_eq!(
                held.as_ref(),
                want.as_ref(),
                "ownership model mismatch, wave {wave}: {issue:?}"
            );
        }
    }
}

/// An independent blocker oracle over `details` (every generated issue is a task): an issue is
/// blocked by an issue it depends on with `blocks` that is not done, and done means closed or a
/// tombstone, with its status undisputed. Ready are the open issues that are not blocked, not
/// claimed and whose status is undisputed.
pub(super) fn expected_ready(wave: usize, details: &Json) -> BTreeSet<String> {
    let issues = details.as_array().expect("bead show prints a list");
    let by_id: BTreeMap<String, &Json> = issues.iter().map(|issue| (id(issue), issue)).collect();
    let done = |issue: &Json| {
        matches!(issue["status"].as_str(), Some("closed" | "tombstone")) && !status_disputed(issue)
    };
    let blocks = |edge: &Json| {
        let blocker = edge["id"].as_str().and_then(|key| by_id.get(key));
        edge["dependency_type"].as_str() == Some("blocks") && blocker.is_some_and(|b| !done(b))
    };
    let mut ready = BTreeSet::new();
    for (key, issue) in &by_id {
        let edges = issue["dependencies"].as_array().unwrap_or_default();
        let blocked = edges.iter().any(blocks);
        let shown = issue["blocked"].as_bool();
        let mismatch = format!("wave {wave}: {key} blocker oracle mismatch");
        assert_eq!(shown, Some(blocked), "{mismatch}");
        let open = issue["status"].as_str() == Some("open");
        if open && !blocked && !issue["claim"].truthy() && !status_disputed(issue) {
            ready.insert(key.clone());
        }
    }
    ready
}

/// One machine after a wave: it offers exactly `ready`, reports no integrity problem and no event
/// waiting, and holds no unconfirmed marker; the number of events it has.
pub(super) fn settled(wave: usize, machine: &Machine, ready: &BTreeSet<String>) -> i64 {
    let name = machine.name();
    let offered = machine.ready();
    assert_eq!(&offered, ready, "wave {wave}: {name} ready mismatch");
    let info = machine.bead(["info"]).expect("an info report");
    let quiet = !info["integrity"].truthy() && !info["waiting_events"].truthy();
    assert!(quiet, "wave {wave}: {name} {info:?}");
    let marker = json_file(&machine.store().join("unconfirmed.json"));
    assert!(!marker.is_some_and(|m| m.truthy()), "wave {wave}: {name}");
    info["events"].as_i64().expect("a count of events")
}
