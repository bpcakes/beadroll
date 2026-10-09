//! `ReplayAuditProperties`, from the first independent audit (AUDIT), 2026-10-06: replay does not
//! depend on delivery order, registers keep concurrent writes, labels and dependencies are
//! add-wins, and missing causes hold what follows them. The audit's fixture is [`fixture`]. The
//! workspace-isolation test needs the store and lands with Phase 2a.

pub(crate) mod fixture;

use std::collections::BTreeSet;

use beadroll_core::event::{Field, REGISTERS, REQUIRED_FIELDS};
use beadroll_core::journal::Journal;
use beadroll_core::op::PyInt;
use beadroll_core::query::Edges;
use beadroll_core::state::State;
use beadroll_test_support::{Json, json};

use fixture::{Fixture, MISSING, conflict, fields, issue, snapshot};

/// A seeded generator (SplitMix64): the Python test draws from `random.Random(6403)`.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        usize::try_from((z ^ (z >> 31)) % u64::try_from(n).unwrap()).unwrap()
    }
}

/// Every order of `n` things.
fn permutations(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for shorter in permutations(n - 1) {
        for at in 0..=shorter.len() {
            let mut order = shorter.clone();
            order.insert(at, n - 1);
            out.push(order);
        }
    }
    out
}

#[test]
fn six_machine_3000_event_replay_all_720_enumerations() {
    let mut f = Fixture::new();
    let mut items = vec![f.issue.clone()];
    for n in 1..30 {
        items.push(f.create(&format!("au-{n}")));
    }
    for item in items.iter().take(5) {
        let last = f.last();
        f.event(
            "claim",
            Some(item),
            "seed",
            &[&last],
            json!({"mode": "online", "generation": 1}),
        );
    }
    let replicas = ["seed", "m1", "m2", "m3", "m4", "m5"];
    let mut heads = vec![f.last(); replicas.len()];
    let mut rng = Rng(6403);
    for index in 0..3000 {
        let replica = index % 6;
        let issue = items[rng.below(items.len())].clone();
        let mut deps = BTreeSet::from([heads[replica].clone()]);
        if index % 13 == 0 {
            deps.insert(heads[rng.below(6)].clone());
        }
        let (op, data) = match index % 8 {
            0 => (
                "set",
                json!({"fields": {"priority": (i64::try_from(rng.below(5)).unwrap())}}),
            ),
            1 => (
                "set",
                json!({"fields": {"notes": (format!("note {index}"))}}),
            ),
            2 => (
                "label_add",
                json!({"labels": [(format!("tag{}", index % 5))]}),
            ),
            3 => (
                "label_remove",
                json!({"labels": [(format!("tag{}", index % 5))]}),
            ),
            4 => ("claim", json!({"mode": "offline"})),
            5 => ("comment", json!({"text": (format!("comment {index}"))})),
            6 => (
                "dep_add",
                json!({"to": (items[rng.below(items.len())].as_str()), "type": "related"}),
            ),
            _ => (
                "dep_remove",
                json!({"to": (items[rng.below(items.len())].as_str()), "type": "related"}),
            ),
        };
        let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
        heads[replica] = f.event(op, Some(&issue), replicas[replica], &deps, data);
    }
    let grouped: Vec<Vec<&Json>> = replicas
        .iter()
        .map(|r| {
            f.events
                .iter()
                .filter(|e| e.get("replica").and_then(Json::as_str) == Some(r))
                .collect()
        })
        .collect();
    let expected = snapshot(f.journal());
    for order in permutations(replicas.len()) {
        // Repeat delivery, including seed events, while preserving each journal's append order.
        let mut j = Journal::new();
        for &replica in order.iter().chain(order.iter().rev()) {
            for event in &grouped[replica] {
                j.add((*event).clone(), replicas[replica], "", true);
            }
        }
        assert_eq!(snapshot(j), expected, "{order:?}");
    }
    for _ in 0..16 {
        let mut j = Journal::new();
        let mut remaining: Vec<(usize, usize)> = (0..replicas.len()).map(|r| (r, 0)).collect();
        while !remaining.is_empty() {
            let at = rng.below(remaining.len());
            let (replica, next) = remaining[at];
            let Some(event) = grouped[replica].get(next) else {
                remaining.remove(at);
                continue;
            };
            remaining[at].1 += 1;
            j.add((*event).clone(), replicas[replica], "", true);
            if rng.below(2) == 1 {
                j.add((*event).clone(), replicas[replica], "", true);
            }
        }
        assert_eq!(snapshot(j), expected);
    }
}

#[test]
fn missing_causes_hold_claim_and_close_of_pending_create() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let predecessor = f.event(
        "comment",
        Some(&uid),
        "a",
        &[MISSING],
        json!({"text": "waiting"}),
    );
    let claim = f.event(
        "claim",
        Some(&uid),
        "a",
        &[&predecessor],
        json!({"mode": "online", "generation": 1}),
    );
    let create = f.event(
        "create",
        None,
        "b",
        &[MISSING],
        json!({"key": "au-pending"}),
    );
    let close = f.event("close", Some(&create), "b", &[&create], json!({}));
    let st = f.state();
    let pending: BTreeSet<String> = st.pending().iter().map(ToString::to_string).collect();
    assert_eq!(
        pending,
        BTreeSet::from([predecessor, claim, create.clone(), close])
    );
    assert!(issue(&st, &uid).claim().is_none());
    assert!(st.issue_named(&create).is_none());
}

#[test]
fn all_registers_keep_concurrent_clear_and_value_then_settle() {
    for field in REGISTERS {
        let mut f = Fixture::new();
        let uid = f.issue.clone();
        if REQUIRED_FIELDS.contains(&field) {
            // Q09: clearing a required field is a validation error, not a write.
            let before = issue(&f.state(), &uid).field(field).cloned();
            let bad = f.event("set", Some(&uid), "a", &[&uid], fields(field, Json::Null));
            let st = f.state();
            assert!(
                st.integrity()
                    .iter()
                    .any(|row| row.event.as_deref() == Some(bad.as_str())),
                "{field:?}"
            );
            assert_eq!(issue(&st, &uid).field(field).cloned(), before, "{field:?}");
            continue;
        }
        let a = f.event("set", Some(&uid), "a", &[&uid], fields(field, Json::Null));
        let value = if matches!(field, Field::Priority | Field::EstimatedMinutes) {
            json!(4)
        } else {
            json!("value")
        };
        let b = f.event("set", Some(&uid), "b", &[&uid], fields(field, value));
        let st = f.state();
        assert_eq!(
            conflict(&st, issue(&st, &uid), field).map(|c| c.len()),
            Some(2),
            "{field:?}"
        );
        let register = issue(&st, &uid).registers().get(&field).unwrap();
        assert!(register.values().any(|v| *v == Json::Null), "{field:?}");
        f.event("set", Some(&uid), "a", &[&a, &b], fields(field, Json::Null));
        let st = f.state();
        assert!(
            conflict(&st, issue(&st, &uid), field).is_none(),
            "{field:?}"
        );
        assert!(issue(&st, &uid).field(field).is_none(), "{field:?}");
    }
}

#[test]
fn three_machine_add_wins_labels_and_dependencies() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let target = f.create("au-target");
    let edge = (target.clone(), "blocks".to_owned());
    for (op, removal, payload) in [
        ("label_add", "label_remove", json!({"labels": ["urgent"]})),
        (
            "dep_add",
            "dep_remove",
            json!({"to": (target.as_str()), "type": "blocks"}),
        ),
    ] {
        let last = f.last();
        let a = f.event(op, Some(&uid), "a", &[&last], payload.clone());
        let b = f.event(op, Some(&uid), "b", &[&a], payload.clone());
        let c = f.event(removal, Some(&uid), "c", &[&a], payload.clone());
        let held = |st: &State| {
            let issue = issue(st, &uid);
            if op == "label_add" {
                issue.labels().contains("urgent")
            } else {
                issue.deps().contains_key(&edge)
            }
        };
        assert!(held(&f.state()), "{op}");
        f.event(removal, Some(&uid), "c", &[&b, &c], payload);
        assert!(!held(&f.state()), "{op}");
    }
}

#[test]
fn cycle_after_merge_and_partial_removal() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let other = f.create("au-two");
    let a = f.event(
        "dep_add",
        Some(&uid),
        "a",
        &[&other],
        json!({"to": (other.as_str()), "type": "blocks"}),
    );
    let b = f.event(
        "dep_add",
        Some(&other),
        "b",
        &[&other],
        json!({"to": (uid.as_str()), "type": "blocks"}),
    );
    let st = f.state();
    assert_eq!(st.cycles(Edges::WaitsFor).len(), 1);
    assert!(st.issues().all(|i| st.is_blocked(i)));
    f.event(
        "dep_remove",
        Some(&other),
        "b",
        &[&a, &b],
        json!({"to": (uid.as_str()), "type": "blocks"}),
    );
    let st = f.state();
    assert!(st.cycles(Edges::WaitsFor).is_empty());
    assert!(!st.is_blocked(issue(&st, &other)));
    assert!(st.is_blocked(issue(&st, &uid)));
}

#[test]
fn tombstoned_blocker_unblocks_unless_status_disputed() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let child = f.create("au-child");
    let edge = f.event(
        "dep_add",
        Some(&child),
        "seed",
        &[&child],
        json!({"to": (uid.as_str()), "type": "blocks"}),
    );
    f.event("delete", Some(&uid), "a", &[&edge], json!({}));
    let st = f.state();
    assert!(!st.is_blocked(issue(&st, &child)));
    f.event("reopen", Some(&uid), "b", &[&edge], json!({}));
    let st = f.state();
    assert!(conflict(&st, issue(&st, &uid), Field::Status).is_some());
    assert!(st.is_blocked(issue(&st, &child)));
}

#[test]
fn handwritten_offline_takeover_does_not_fence_confirmed_owner() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let claim = f.event(
        "claim",
        Some(&uid),
        "a",
        &[&uid],
        json!({"mode": "online", "generation": 1}),
    );
    f.event(
        "claim",
        Some(&uid),
        "b",
        &[&claim],
        json!({"mode": "offline", "takeover": (claim.as_str())}),
    );
    f.event(
        "close",
        Some(&uid),
        "a",
        &[&claim],
        json!({"claim": (claim.as_str())}),
    );
    let st = f.state();
    let issue = issue(&st, &uid);
    assert!(issue.claim().is_none());
    assert_eq!(issue.status(), "closed");
    assert!(issue.stale().is_empty());
}

#[test]
fn causal_write_wins_despite_backwards_clock() {
    let mut f = Fixture::new();
    let uid = f.issue.clone();
    let a = f.event(
        "set",
        Some(&uid),
        "a",
        &[&uid],
        json!({"fields": {"priority": 1}}),
    );
    let b = f.event(
        "set",
        Some(&uid),
        "b",
        &[&a],
        json!({"fields": {"priority": 4}}),
    );
    f.edit(&b, |event| {
        drop(event.insert("id".into(), json!("01000000-0000-7000-8000-000000000001")))
    });
    let st = f.state();
    assert_eq!(issue(&st, &uid).priority(), PyInt::from(4));
    assert!(conflict(&st, issue(&st, &uid), Field::Priority).is_none());
}
