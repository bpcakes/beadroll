//! Dependency cycles are found whole: each elementary cycle, the one left after a partial removal,
//! and every small graph as a brute-force oracle finds them.

use std::collections::BTreeSet;

use beadroll_core::query::{Edges, find_cycles};
use beadroll_test_support::{Json, json, json_id};

use super::builders::{actors, id_of, serial};

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
fn cycle_enumerator_matches_every_graph_through_four_nodes() {
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

/// `dep cycles` lists up to 200 elementary cycles: a complete digraph on six nodes has 409, and
/// the enumerator stops at 200, each a real one and none twice.
#[test]
fn cycle_enumerator_stops_at_two_hundred() {
    let size = 6;
    let edges: Vec<BTreeSet<usize>> = (0..size)
        .map(|source| (0..size).filter(|target| *target != source).collect())
        .collect();
    let every = brute_force(size, &edges);
    assert_eq!(every.len(), 409);
    let nodes: Vec<String> = (0..size).map(|n| n.to_string()).collect();
    let follow = |uid: &String| -> Vec<String> {
        let source = uid.parse::<usize>().unwrap();
        edges[source].iter().map(usize::to_string).collect()
    };
    let found = find_cycles(&nodes, &follow);
    assert_eq!(found.len(), 200);
    let distinct: BTreeSet<Vec<String>> = found.into_iter().collect();
    assert_eq!(distinct.len(), 200, "a cycle listed twice");
    assert!(
        distinct.is_subset(&every),
        "a listed cycle is not an elementary one"
    );
}

#[test]
fn cycle_scan_reports_each_elementary_cycle() {
    let mut f = actors::Fixture::new();
    let init = f.event(
        "init",
        "a",
        &[],
        actors::Target::Itself,
        json!({"prefix": "ex"}),
    );
    let mut events = vec![init];
    let mut last = json_id(&events[0]);
    let mut issues = Vec::new();
    for key in ["a", "b", "c"] {
        let data = json!({"key": (format!("ex-{key}")), "fields": {"title": key}});
        let create = f.event("create", "a", &[&last], actors::Target::Itself, data);
        last = json_id(&create);
        issues.push(last.clone());
        events.push(create);
    }
    for (source, target) in [(0, 1), (0, 2), (1, 2), (2, 0)] {
        let data = json!({"to": (issues[target].as_str()), "type": "blocks"});
        let edge = f.event(
            "dep_add",
            "a",
            &[&last],
            actors::Target::Named(&issues[source]),
            data,
        );
        last = json_id(&edge);
        events.push(edge);
    }
    let rows: Vec<(&Json, &str)> = events.iter().map(|event| (event, "a")).collect();
    let cycles: BTreeSet<Vec<String>> = actors::replayed(&rows)
        .cycles(Edges::WaitsFor)
        .into_iter()
        .collect();
    let expected: BTreeSet<Vec<String>> = [vec!["ex-a", "ex-b", "ex-c"], vec!["ex-a", "ex-c"]]
        .into_iter()
        .map(|cycle| cycle.into_iter().map(str::to_owned).collect())
        .collect();
    assert_eq!(cycles, expected);
}

/// Two elementary cycles share an edge; removing a leaf edge of one leaves the other reported, and
/// every issue on it blocked.
#[test]
fn merged_dependency_cycle_reports_remaining_cycle_after_partial_removal() {
    let mut f = serial::Fixture::new();
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
    let before = serial::replayed(&events);
    assert_eq!(before.cycles(Edges::WaitsFor).len(), 2);
    let data = json!({"to": (id_of(&first)), "type": "blocks"});
    let removed = f.event("dep_remove", "a", &[&previous], Some(id_of(&second)), data);
    let after = serial::replayed(&[events, vec![removed]].concat());
    assert_eq!(
        after.cycles(Edges::WaitsFor),
        [["ex-one", "ex-two", "ex-three"]]
    );
    assert!(after.issues().all(|issue| after.is_blocked(issue)));
}
