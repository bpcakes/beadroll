//! `ReplayResiduals`, from the third independent review (REVIEW3), 2026-10-06: the residual
//! probes of replay's scoping, ownership and cycle scan.

use std::collections::BTreeSet;

use beadroll_core::journal::Journal;
use beadroll_core::journal::JournalOrder;
use beadroll_core::query::Edges;
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_test_support::Json;
use beadroll_test_support::events::Event;
use beadroll_test_support::json;

use super::replay_reaudit::{id, row};

/// Whose issue an event names: itself, another, or none (no `issue` member at all).
#[derive(Clone, Copy)]
enum Target<'a> {
    Itself,
    Named(&'a str),
    Omitted,
}

/// Events with fixed UUIDv7s, numbered in the order they are made.
struct Fixture {
    serial: u64,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture { serial: 0 }
    }

    fn event(
        &mut self,
        op: &str,
        replica: &str,
        deps: &[&str],
        issue: Target<'_>,
        data: Json,
    ) -> Json {
        self.serial += 1;
        let eid = format!("019a3333-3333-7000-8000-{:012x}", self.serial);
        let event = Event::new(&eid, op)
            .replica(replica)
            .deps(deps)
            .actor(json!({"agent": "codex", "thread": (format!("{replica}-thread"))}))
            .data(data);
        match issue {
            Target::Itself => event.json(),
            Target::Named(uid) => event.issue(uid).json(),
            Target::Omitted => event.without("issue").json(),
        }
    }

    /// The tracker's init, and one issue, `ex-one`, titled `survivor`.
    fn base(&mut self) -> (Json, Json) {
        let init = self.event("init", "a", &[], Target::Itself, json!({"prefix": "ex"}));
        let create = self.event(
            "create",
            "a",
            &[&id(&init)],
            Target::Itself,
            json!({"key": "ex-one", "fields": {"title": "survivor"}}),
        );
        (init, create)
    }
}

/// A journal of `rows`, each event in the journal of the machine named beside it.
fn add_all(rows: &[(&Json, &str)]) -> Journal {
    let mut journal = Journal::new();
    for (event, physical_replica) in rows {
        journal.add(
            (*event).clone(),
            physical_replica,
            "",
            JournalOrder::Sequenced,
        );
    }
    journal
}

fn replayed(rows: &[(&Json, &str)]) -> State {
    replay(add_all(rows)).unwrap()
}

#[test]
fn missing_issue_creates_cannot_alias_and_overwrite() {
    let mut f = Fixture::new();
    let init = f.event("init", "a", &[], Target::Itself, json!({"prefix": "ex"}));
    let first = f.event(
        "create",
        "a",
        &[&id(&init)],
        Target::Omitted,
        json!({"key": "ex-first", "fields": {"title": "first"}}),
    );
    let second = f.event(
        "create",
        "b",
        &[&id(&first)],
        Target::Omitted,
        json!({"key": "ex-second", "fields": {"title": "second"}}),
    );
    let state = replayed(&[(&init, "a"), (&first, "a"), (&second, "b")]);
    // Either normalize omitted create UIDs to their event IDs or reject the malformed creates. They
    // must never share a missing UID and overwrite (issues are keyed by event id here, so none can).
    if state.issue_named(&id(&first)).is_some() || state.issue_named(&id(&second)).is_some() {
        assert_eq!(state.resolve("ex-first").unwrap().title(), "first");
        assert_eq!(state.resolve("ex-second").unwrap().title(), "second");
    } else {
        assert!(state.integrity().len() >= 2);
    }
}

#[test]
fn identical_misplaced_duplicate_does_not_change_broken_chain_result() {
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let uid = id(&create);
    let first = f.event(
        "set",
        "a",
        &[&uid],
        Target::Named(&uid),
        json!({"fields": {"title": "first"}}),
    );
    let second = f.event(
        "set",
        "a",
        &[&uid],
        Target::Named(&uid),
        json!({"fields": {"title": "second"}}),
    );
    let projection = |misplaced_first: bool| {
        let mut rows = vec![(&init, "a"), (&create, "a")];
        if misplaced_first {
            rows.push((&second, "b"));
        }
        rows.extend([(&first, "a"), (&second, "a")]);
        if !misplaced_first {
            rows.push((&second, "b"));
        }
        let state = replayed(&rows);
        let issue = state.issue_named(&uid).unwrap();
        let conflicts = state
            .conflicts(issue)
            .into_iter()
            .map(|(field, alternatives)| {
                let alternatives = alternatives
                    .iter()
                    .map(|a| Json::from(a.to_value()))
                    .collect::<Vec<_>>();
                (field.name().to_owned(), Json::Array(alternatives))
            });
        let breaks = state.chain_breaks().iter().map(|(replica, at)| {
            let at = at
                .iter()
                .map(|n| json!(i64::try_from(*n).unwrap()))
                .collect::<Vec<_>>();
            (replica.clone(), Json::Array(at))
        });
        json!({
            "title": (issue.title()),
            "conflicts": (Json::Object(Box::new(conflicts.collect()))),
            "integrity": (Json::Array(state.integrity().iter().map(row).collect())),
            "chain_breaks": (Json::Object(Box::new(breaks.collect())))
        })
    };
    assert_eq!(projection(true), projection(false));
}

#[test]
fn takeover_must_causally_see_the_claim_it_supersedes() {
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let uid = id(&create);
    let claim = f.event(
        "claim",
        "a",
        &[&uid],
        Target::Named(&uid),
        json!({"mode": "online", "generation": 1}),
    );
    let takeover = f.event(
        "claim",
        "b",
        &[&uid],
        Target::Named(&uid),
        json!({
            "mode": "online",
            "generation": 2,
            "takeover": (id(&claim)),
            "reason": "forged without seeing predecessor"
        }),
    );
    let close = f.event(
        "close",
        "a",
        &[&id(&claim)],
        Target::Named(&uid),
        json!({"claim": (id(&claim)), "reason": "done"}),
    );
    let state = replayed(&[
        (&init, "a"),
        (&create, "a"),
        (&claim, "a"),
        (&takeover, "b"),
        (&close, "a"),
    ]);
    let event = |e: &Json| beadroll_core::id::EventId::parse(&id(e)).unwrap();
    assert!(!state.saw(event(&takeover), event(&claim)));
    let issue = state.issue_named(&uid).unwrap();
    assert_eq!(issue.status(), "closed");
    assert!(issue.claim().is_none());
    assert!(issue.stale().is_empty());
}

#[test]
fn quarantined_common_descendant_dependency_is_scoped() {
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let uid = id(&create);
    let quarantined = f.event(
        "comment",
        "b",
        &[],
        Target::Named(&uid),
        json!({"text": "variant one"}),
    );
    let child = f.event(
        "comment",
        "c",
        &[&id(&quarantined)],
        Target::Named(&uid),
        json!({"text": "descendant survives"}),
    );
    // The quarantined event names its own descendant as its cause, in both variants.
    let mut quarantined = quarantined;
    if let Json::Object(members) = &mut quarantined {
        members.insert("deps".into(), json!([(id(&child))]));
    }
    let mut variant = quarantined.clone();
    if let Json::Object(members) = &mut variant {
        members.insert("data".into(), json!({"text": "variant two"}));
    }
    let state = replayed(&[
        (&init, "a"),
        (&create, "a"),
        (&quarantined, "b"),
        (&child, "c"),
        (&variant, "b"),
    ]);
    assert!(state.pending().is_empty());
    let texts: Vec<Json> = state
        .issue_named(&uid)
        .unwrap()
        .comments()
        .iter()
        .filter_map(|c| c.get("text"))
        .collect();
    assert_eq!(texts, vec![json!("descendant survives")]);
    assert_eq!(state.integrity().len(), 1);
}

#[test]
fn malformed_embedded_create_comment_is_scoped() {
    let mut f = Fixture::new();
    let (init, create) = f.base();
    let malformed = f.event(
        "create",
        "b",
        &[&id(&create)],
        Target::Itself,
        json!({
            "key": "ex-bad",
            "fields": {"title": "malformed"},
            "comments": [{"text": "missing author and created_at"}]
        }),
    );
    let state = replayed(&[(&init, "a"), (&create, "a"), (&malformed, "b")]);
    // A rejected malformed issue is acceptable; so is a fully normalized one. In either case, one
    // bad embedded comment cannot disable global export/detail.
    let records = state.export_records();
    assert!(
        records
            .iter()
            .any(|record| record.get("id") == Some(&json!("ex-one")))
    );
    match state.issue_named(&id(&malformed)) {
        Some(issue) => drop(state.detail(issue)),
        None => assert!(!state.integrity().is_empty()),
    }
}

#[test]
fn cycle_scan_reports_each_elementary_cycle() {
    let mut f = Fixture::new();
    let init = f.event("init", "a", &[], Target::Itself, json!({"prefix": "ex"}));
    let mut events = vec![init];
    let mut last = id(&events[0]);
    let mut issues = Vec::new();
    for key in ["a", "b", "c"] {
        let data = json!({"key": (format!("ex-{key}")), "fields": {"title": key}});
        let create = f.event("create", "a", &[&last], Target::Itself, data);
        last = id(&create);
        issues.push(last.clone());
        events.push(create);
    }
    for (source, target) in [(0, 1), (0, 2), (1, 2), (2, 0)] {
        let data = json!({"to": (issues[target].as_str()), "type": "blocks"});
        let edge = f.event(
            "dep_add",
            "a",
            &[&last],
            Target::Named(&issues[source]),
            data,
        );
        last = id(&edge);
        events.push(edge);
    }
    let rows: Vec<(&Json, &str)> = events.iter().map(|event| (event, "a")).collect();
    let cycles: BTreeSet<Vec<String>> = replayed(&rows)
        .cycles(Edges::WaitsFor)
        .into_iter()
        .collect();
    let expected: BTreeSet<Vec<String>> = [vec!["ex-a", "ex-b", "ex-c"], vec!["ex-a", "ex-c"]]
        .into_iter()
        .map(|cycle| cycle.into_iter().map(str::to_owned).collect())
        .collect();
    assert_eq!(cycles, expected);
}
