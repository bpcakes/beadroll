//! `ReplayReview4`: the fourth independent review (2026-10-07-4-codex), folded in after the fixes. An
//! inert placeholder lends no ancestry to its journal's tail, clearing a required register leaves
//! every query usable, and the cycle enumerator matches a brute-force oracle on every small graph.

use std::collections::BTreeSet;

use beadroll_core::event::Field;
use beadroll_core::id::EventId;
use beadroll_core::journal::Journal;
use beadroll_core::op::IntegerField;
use beadroll_core::query::find_cycles;
use beadroll_core::replay::replay;
use beadroll_test_support::events::Event;
use beadroll_test_support::{Json, json};

/// Event `number` of the class: id `019a4444-4444-7000-8000-<number>`, about `issue` (itself when
/// `None`).
fn event(
    number: u64,
    op: &str,
    replica: &str,
    deps: &[&str],
    issue: Option<&str>,
    data: Json,
) -> Json {
    let id = format!("019a4444-4444-7000-8000-{number:012x}");
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

fn journal(events: &[&Json]) -> Journal {
    let mut journal = Journal::new();
    for event in events {
        let replica = event["replica"].as_str().unwrap().to_owned();
        journal.add((*event).clone(), &replica, "", true);
    }
    journal
}

/// Q07: an inert placeholder must not invent predecessor ancestry for its tail.
#[test]
fn q07_invalid_placeholder_cannot_bridge_a_broken_machine_chain() {
    let init = event(1, "init", "seed", &[], None, json!({"prefix": "ex"}));
    let create = event(
        2,
        "create",
        "seed",
        &[id_of(&init)],
        None,
        json!({"key": "ex-one", "fields": {"title": "one"}}),
    );
    let on = Some(id_of(&create));
    let first = event(
        3,
        "set",
        "a",
        &[id_of(&create)],
        on,
        json!({"fields": {"priority": 1}}),
    );
    let malformed = event(
        4,
        "comment",
        "a",
        &[id_of(&create)],
        on,
        json!({"text": "bad"}),
    );
    let Json::Object(mut members) = malformed else {
        panic!("an event is an object")
    };
    members.insert("actor".into(), json!([])); // not an actor: the event is invalid
    let malformed = Json::Object(members);
    let later = event(
        5,
        "set",
        "a",
        &[id_of(&malformed)],
        on,
        json!({"fields": {"priority": 4}}),
    );
    let state = replay(journal(&[&init, &create, &first, &malformed, &later])).unwrap();
    assert!(!state.saw(uid(&later), uid(&first)));
    assert_eq!(
        state.issue(uid(&create)).unwrap().priority(),
        IntegerField::from(1)
    );
    assert!(state.chain_breaks().contains_key("a"));
}

/// Q09: required-field clears must be rejected or leave every query usable.
#[test]
fn q09_required_register_clears_remain_queryable() {
    for (offset, field) in (10..).zip([Field::Title, Field::Priority, Field::IssueType]) {
        let init = event(offset, "init", "seed", &[], None, json!({"prefix": "ex"}));
        let key = format!("ex-{}", field.name());
        let fields = json!({"title": "one", "priority": 2, "issue_type": "task"});
        let create = event(
            offset + 1,
            "create",
            "seed",
            &[id_of(&init)],
            None,
            json!({"key": (key.as_str()), "fields": (fields.clone())}),
        );
        let mut clear_fields = beadroll_core::json::Object::new();
        clear_fields.insert(field.name().into(), Json::Null);
        let on = Some(id_of(&create));
        let clear = event(
            offset + 2,
            "set",
            "seed",
            &[id_of(&create)],
            on,
            json!({"fields": (Json::from(clear_fields))}),
        );
        let state = replay(journal(&[&init, &create, &clear])).unwrap();
        let issue = state.issue(uid(&create)).unwrap();
        if !state.integrity().is_empty() {
            assert!(
                state
                    .integrity()
                    .iter()
                    .any(|row| row.event.as_deref() == Some(id_of(&clear))),
                "{field:?}"
            );
            assert_eq!(issue.field(field), fields.get(field.name()), "{field:?}");
        }
        // The accepted clear must stay query-safe.
        state.brief(issue);
        state.line(issue);
        state.export_records();
    }
}

/// The elementary cycles of a graph found by walking every path from each node through larger
/// ones, each written from its smallest member.
fn brute_force(size: usize, edges: &[BTreeSet<usize>]) -> BTreeSet<Vec<String>> {
    fn walk(
        start: usize,
        path: &mut Vec<usize>,
        edges: &[BTreeSet<usize>],
        cycles: &mut BTreeSet<Vec<String>>,
    ) {
        let last = *path.last().unwrap();
        for &target in &edges[last] {
            if target == start {
                let cycle: Vec<String> = path.iter().map(usize::to_string).collect();
                let rotations = (0..cycle.len()).map(|at| [&cycle[at..], &cycle[..at]].concat());
                cycles.insert(rotations.min().unwrap());
            } else if !path.contains(&target) && target >= start {
                path.push(target);
                walk(start, path, edges, cycles);
                path.pop();
            }
        }
    }
    let mut cycles = BTreeSet::new();
    for start in 0..size {
        walk(start, &mut vec![start], edges, &mut cycles);
    }
    cycles
}

/// D01 verification: every <=4-node directed graph matches a brute-force oracle.
#[test]
fn d01_cycle_enumerator_matches_every_graph_through_four_nodes() {
    for size in 1..5usize {
        let pairs: Vec<(usize, usize)> = (0..size)
            .flat_map(|s| (0..size).map(move |t| (s, t)))
            .collect();
        for mask in 0..(1u32 << pairs.len()) {
            let mut edges = vec![BTreeSet::new(); size];
            for (bit, (source, target)) in pairs.iter().enumerate() {
                if mask >> bit & 1 == 1 {
                    edges[*source].insert(*target);
                }
            }
            let expected = brute_force(size, &edges);
            // Issues named "0" to "3", each waiting for its targets in sorted order.
            let nodes: Vec<String> = (0..size).map(|n| n.to_string()).collect();
            let follow = |uid: &String| -> Vec<String> {
                let mut targets: Vec<String> = edges[uid.parse::<usize>().unwrap()]
                    .iter()
                    .map(usize::to_string)
                    .collect();
                targets.sort();
                targets
            };
            let actual: BTreeSet<Vec<String>> = find_cycles(&nodes, &follow).into_iter().collect();
            assert_eq!(actual, expected, "{size} {mask} {edges:?}");
        }
    }
}
