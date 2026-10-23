//! AD01–AD02: stored key text and native claim counters keep their separate boundaries.

use beadroll_core::id::EventId;
use beadroll_core::journal::Journal;
use beadroll_core::json::Int;
use beadroll_core::projection::{Extended, Record, project};
use beadroll_core::replay::replay;
use beadroll_test_support::events::{Event, id, journal};
use beadroll_test_support::{Json, json};

#[test]
fn ad01_a_long_key_written_by_the_old_importer_still_replays() {
    // Unmodified journal bytes from 9e27e78's `init --prefix x --import issues.jsonl` on a
    // synthetic local remote, recorded 2026-10-23. The source id is x. followed by forty 1s.
    let mut journal = Journal::new();
    journal.add_segment(
        include_bytes!("numeric_limits/legacy-import.jsonl"),
        "e28233b5",
        "events/e28233b5/000001.jsonl",
    );
    let state = replay(journal).unwrap();
    assert!(state.integrity().is_empty(), "{:?}", state.integrity());
    let key = format!("x.{}", "1".repeat(40));
    let issue = state.resolve(&key).unwrap();
    assert_eq!(issue.key(), key);
    assert_eq!(state.issues().count(), 1);
}

fn next(n: u64, op: &str, data: Json) -> Json {
    Event::new(&id(n), op)
        .deps(&[&id(n - 1)])
        .issue(&id(2))
        .data(data)
        .json()
}

fn claimed(generation: i128) -> Vec<Json> {
    vec![
        Event::new(&id(1), "init")
            .data(json!({"prefix": "ex"}))
            .json(),
        next(
            2,
            "create",
            json!({"key": "ex-1", "fields": {"title": "kept"}}),
        ),
        next(
            3,
            "claim",
            json!({"generation": (Json::Int(Int::from_i128(generation)))}),
        ),
    ]
}

#[test]
fn ad02_exhausted_reclaim_reports_integrity_without_changing_the_issue() {
    let mut events = claimed(i128::MAX);
    events.push(next(4, "release", json!({"claim": (id(3))})));
    let before = replay(journal(&events)).unwrap();
    let saved = project(journal(&events)).unwrap();
    events.push(next(5, "claim", json!({})));
    let after = replay(journal(&events)).unwrap();
    let uid = EventId::parse(&id(2)).unwrap();
    assert_eq!(
        before.detail(before.issue(uid).unwrap()),
        after.detail(after.issue(uid).unwrap())
    );
    assert_eq!(after.integrity().len(), 1);
    assert_eq!(after.integrity()[0].event.as_deref(), Some(id(5).as_str()));
    assert_eq!(
        after.integrity()[0].problem,
        "claim generation is beyond what bead counts"
    );
    // The projection's shared application path must give the same refusal, or ask for replay.
    if let Extended::Applied(extended) = saved
        .extend(vec![Record {
            value: events[4].clone(),
            replica: "a".into(),
            origin: "native".into(),
            sequenced: true,
        }])
        .unwrap()
    {
        assert_eq!(extended.state().integrity(), after.integrity());
        assert_eq!(
            extended
                .state()
                .detail(extended.state().issue(uid).unwrap()),
            after.detail(after.issue(uid).unwrap())
        );
    }
}

#[test]
fn ad02_exhausted_takeover_keeps_the_owner_and_does_not_silence_its_release() {
    // Reach MAX both by proposing it and by incrementing MAX-1, so refusal cannot depend on
    // looking only at the previous claim's payload.
    for direct in [true, false] {
        let mut events = claimed(if direct { i128::MAX } else { i128::MAX - 1 });
        let claim = if direct {
            3
        } else {
            events.push(next(4, "release", json!({"claim": (id(3))})));
            events.push(next(5, "claim", json!({})));
            5
        };
        let before = replay(journal(&events)).unwrap();
        let takeover = Event::new(&id(claim + 1), "claim")
            .replica("b")
            .deps(&[&id(claim)])
            .issue(&id(2))
            .data(json!({"takeover": (id(claim)), "reason": "handoff"}))
            .json();
        events.push(takeover);
        let after = replay(journal(&events)).unwrap();
        let uid = EventId::parse(&id(2)).unwrap();
        assert_eq!(
            before.detail(before.issue(uid).unwrap()),
            after.detail(after.issue(uid).unwrap())
        );
        assert_eq!(after.integrity().len(), 1);
        // The old owner's later release did not see the takeover. A valid takeover would
        // supersede it globally; an exhausted one must leave it effective.
        events.push(
            Event::new(&id(claim + 2), "release")
                .deps(&[&id(claim)])
                .issue(&id(2))
                .data(json!({"claim": (id(claim))}))
                .json(),
        );
        let released = replay(journal(&events)).unwrap();
        let issue = released.issue(uid).unwrap();
        assert!(issue.claim().is_none());
        assert_eq!(issue.status(), "open");
        assert_eq!(issue.claim_history().last().unwrap().how, "release");
        assert_eq!(released.integrity().len(), 1);
    }
}

#[test]
fn ae01_exhaustion_retry_keeps_a_missing_takeover_target_unsettled() {
    let mut events = claimed(i128::MAX);
    events.push(next(4, "claim", json!({"takeover": (id(3))})));
    events.push(next(5, "claim", json!({"takeover": (id(99))})));
    let saved = project(journal(&events)).unwrap();
    assert!(!saved.extendable());
    let problem = |state: &beadroll_core::state::State| {
        state
            .integrity()
            .iter()
            .find(|entry| entry.event.as_deref() == Some(id(5).as_str()))
            .unwrap()
            .problem
            .clone()
    };
    assert_eq!(
        problem(saved.state()),
        "takeover names a claim that is not a claim on this issue"
    );
    // Arrival changes why the takeover was refused. The projection must ask for replay rather
    // than keep the earlier diagnostic alongside an otherwise successfully applied record.
    let arrived = Event::new(&id(99), "claim")
        .deps(&[&id(5)])
        .issue(&id(2))
        .json();
    events.push(arrived.clone());
    let full = replay(journal(&events)).unwrap();
    assert_eq!(
        problem(&full),
        "takeover of a claim its writer had not seen"
    );
    assert!(matches!(
        saved
            .extend(vec![Record {
                value: arrived,
                replica: "a".into(),
                origin: "native".into(),
                sequenced: true,
            }])
            .unwrap(),
        Extended::Replay
    ));
}
