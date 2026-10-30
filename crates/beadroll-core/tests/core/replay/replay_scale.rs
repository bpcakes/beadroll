//! `ReplayScale`: replay stays fast and small where ownership history is long.

use std::time::{Duration, Instant};

use beadroll_core::id::EventId;
use beadroll_core::journal::Journal;
use beadroll_core::journal::JournalOrder;
use beadroll_core::replay::replay;
use beadroll_test_support::events::id_at;
use beadroll_test_support::{Json, json};

/// P01: ancestry over 3,000 alternating claim/release events remains efficient. Check `saw` in
/// both directions across the history and bound replay time; the ancestry representation is
/// private to core.
#[test]
fn ownership_ancestry_stays_small_on_a_long_claim_release_history() {
    let mut events: Vec<Json> = Vec::new();
    let (mut uid, mut active) = (String::new(), String::new());
    for i in 0..3000u64 {
        let id = id_at(1_770_000_000_000 + i, 0);
        let (op, data, issue) = match i {
            0 => ("init", json!({"prefix": "ex"}), id.clone()),
            1 => {
                uid = id.clone();
                (
                    "create",
                    json!({"key": "ex-1", "fields": {"title": "t"}}),
                    id.clone(),
                )
            }
            _ if i % 2 == 0 => {
                active = id.clone();
                let generation = i64::try_from(i / 2).unwrap();
                (
                    "claim",
                    json!({"mode": "online", "generation": generation}),
                    uid.clone(),
                )
            }
            _ => ("release", json!({"claim": (active.as_str())}), uid.clone()),
        };
        let deps = events.last().map_or_else(
            || json!([]),
            |previous: &Json| json!([(previous["id"].clone())]),
        );
        let event = json!({
            "v": 1, "id": (id.as_str()), "replica": "m", "actor": {"agent": "x", "thread": "t"},
            "deps": deps, "op": op, "issue": (issue.as_str()), "data": data
        });
        events.push(event);
    }
    let mut journal = Journal::new();
    for event in &events {
        let sequenced = matches!(event["op"].as_str(), Some("init" | "create"));
        journal.add(
            event.clone(),
            "m",
            "",
            if sequenced {
                JournalOrder::Sequenced
            } else {
                JournalOrder::Attempt
            },
        );
    }
    let started = Instant::now();
    let state = replay(journal).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert!(state.integrity().is_empty());
    let at = |n: usize| EventId::parse(events[n]["id"].as_str().unwrap()).unwrap();
    assert!(state.saw(at(events.len() - 1), at(2)));
    assert!(!state.saw(at(2), at(events.len() - 1)));
    let issue = state.issue(EventId::parse(&uid).unwrap()).unwrap();
    assert!(issue.claim().is_none()); // the history ends on a release
}
