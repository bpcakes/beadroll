//! One issue given many edges (roll-lqjj.21): each edge it gains, loses or is asked for is found
//! by looking at the edges on its target alone, however the edges come — an import naming keys
//! that have their issues, an import whose keys come one create at a time, `dep add` and `dep
//! remove` by target and by key, a projection read back and extended in one batch or a record at a
//! time — where comparing each with every edge the issue had made N edges cost N².

use crate::json::{Object, Value};
use crate::projection::{Projection, Record};
use crate::replay::replay;
use crate::state::State;

use super::super::{Work, counted};
use super::{History, SMALL, applied, grows_linearly, id, key, one_at_a_time};

/// The history of `issues` creates, the last importing a dependency on the key of every other,
/// which all have their issues: its edges at once.
fn has_many(issues: usize) -> History {
    let mut history = History::new();
    for n in 1..issues {
        history.create(&key(n), &[], &[]);
    }
    let named: Vec<(String, &str)> = (1..issues).map(|n| (key(n), "blocks")).collect();
    history.create(&key(issues), &named, &[]);
    history
}

/// [`has_many`], and then each of the last issue's imported edges removed by the key it named, of
/// its type, as `bead dep remove` removes an imported edge.
fn unbound(issues: usize) -> History {
    let mut history = History::new();
    for n in 1..issues {
        history.create(&key(n), &[], &[]);
    }
    let named: Vec<(String, &str)> = (1..issues).map(|n| (key(n), "blocks")).collect();
    let hub = history.create(&key(issues), &named, &[]);
    for n in 1..issues {
        let mut data = Object::new();
        data.insert("key".into(), Value::from(key(n)));
        data.insert("type".into(), Value::from("blocks"));
        history.add("dep_remove", Some(hub), data);
    }
    history
}

/// The history of `issues` creates, the first importing a dependency on the key of every other,
/// each made later: its edges one create at a time.
fn awaits_many(issues: usize) -> History {
    let mut history = History::new();
    let named: Vec<(String, &str)> = (2..=issues).map(|n| (key(n), "blocks")).collect();
    history.create(&key(1), &named, &[]);
    for n in 2..=issues {
        history.create(&key(n), &[], &[]);
    }
    history
}

/// What `dep_add` and `dep_remove` carry of an edge on `to` of `kind`.
fn edge(to: &str, kind: Option<&str>) -> Object {
    let mut data = Object::new();
    data.insert("to".into(), Value::from(to));
    if let Some(kind) = kind {
        data.insert("type".into(), Value::from(kind));
    }
    data
}

/// The kind of the `n`-th edge.
fn kind(n: usize) -> &'static str {
    ["blocks", "related", "parent-child"][n % 3]
}

/// The history of a hub and `edges` targets, every other one an issue and the rest references
/// that name none: the hub given an edge on each by `dep add`, each removed by `dep remove`, of its
/// kind or of any, and each added again. Returns it and the hub's create.
fn added_removed_and_added_again(edges: usize) -> (History, u64) {
    let mut history = History::new();
    let hub = history.create("ex-hub", &[], &[]);
    let mut targets = Vec::new();
    for n in 0..edges {
        targets.push(if n % 2 == 0 {
            id(history.create(&key(n), &[], &[]))
        } else {
            format!("jira:OPS-{n}")
        });
    }
    for (n, to) in targets.iter().enumerate() {
        history.add("dep_add", Some(hub), edge(to, Some(kind(n))));
    }
    for (n, to) in targets.iter().enumerate() {
        let typed = (n % 4 < 2).then(|| kind(n));
        history.add("dep_remove", Some(hub), edge(to, typed));
    }
    for (n, to) in targets.iter().enumerate() {
        history.add("dep_add", Some(hub), edge(to, Some(kind(n))));
    }
    (history, hub)
}

/// How many edges the issue under `key` has, and how many of them hold more than one addition.
fn edges_of(state: &State, key: &str) -> (usize, usize) {
    let issue = state.issues().find(|issue| issue.key() == key).unwrap();
    let readded = issue.dep_tags().values().filter(|tags| tags.len() > 1);
    (issue.deps().len(), readded.count())
}

#[test]
fn an_issue_given_many_edges_by_one_import_looks_at_each_once() {
    let (state, work) = counted(|| replay(has_many(SMALL).journal()).unwrap());
    assert_eq!(edges_of(&state, &key(SMALL)), (SMALL - 1, 0));
    // each edge it gains is counted, so that the guards below count something to grow
    assert!(work.of(Work::Probe) >= SMALL - 1, "{work:?}");
    grows_linearly(
        "replaying an import that names the key of every other issue",
        &[Work::Probe],
        |issues| has_many(issues).journal(),
        |journal| drop(replay(journal).unwrap()),
    );
}

#[test]
fn an_issues_imported_edges_removed_by_key_are_each_looked_at_once() {
    let state = unbound(SMALL).state();
    assert_eq!(
        edges_of(&state, &key(SMALL)),
        (0, 0),
        "every import removed"
    );
    grows_linearly(
        "replaying an import naming every other issue's key, each then removed by its key",
        &[Work::Probe],
        |issues| unbound(issues).journal(),
        |journal| drop(replay(journal).unwrap()),
    );
}

#[test]
fn an_issue_whose_import_awaits_many_keys_binds_each_by_looking_at_it_once() {
    let state = replay(awaits_many(SMALL).journal()).unwrap();
    assert_eq!(edges_of(&state, &key(1)), (SMALL - 1, 0));
    grows_linearly(
        "replaying an import awaiting the key of every other issue, each made later",
        &[Work::Probe],
        |issues| awaits_many(issues).journal(),
        |journal| drop(replay(journal).unwrap()),
    );
}

#[test]
fn edges_added_removed_and_added_again_to_one_issue_are_each_looked_at_once() {
    let (history, _) = added_removed_and_added_again(SMALL);
    let state = history.state();
    // every edge back, and only the second addition behind each
    assert_eq!(edges_of(&state, "ex-hub"), (SMALL, 0));
    grows_linearly(
        "adding edges on issues and references to one issue, removing each and adding it again",
        &[Work::Probe],
        |edges| added_removed_and_added_again(edges).0.journal(),
        |journal| drop(replay(journal).unwrap()),
    );
}

/// A projection of a hub with `edges` edges, read back as a store reads it, the hub's texts read
/// in to take more, and the records of `edges` more edges for it, each on an issue of its own.
fn read_back_with_more(edges: usize) -> (Projection, Vec<Record>) {
    let mut history = History::new();
    let hub = history.create("ex-hub", &[], &[]);
    let targets: Vec<String> = (0..2 * edges)
        .map(|n| id(history.create(&key(n), &[], &[])))
        .collect();
    for to in &targets[..edges] {
        history.add("dep_add", Some(hub), edge(to, Some("blocks")));
    }
    let held = history.events.len();
    let (encoded, _) = history.projection().encode(0);
    let texts = encoded.texts.clone();
    let mut projection = Projection::decode(encoded).unwrap();
    for to in &targets[edges..] {
        history.add("dep_add", Some(hub), edge(to, Some("blocks")));
    }
    let records = history.after(held);
    let about = projection.about(&records);
    let state = projection.state_mut();
    for (uid, place) in state.wanted_texts(about) {
        let at = usize::try_from(place.at).unwrap();
        let bytes = &texts[at..][..place.len as usize];
        state.read_texts(uid, place, bytes).unwrap();
    }
    (projection, records)
}

#[test]
fn a_projection_read_back_gives_one_issue_many_more_edges_looking_at_each_once() {
    let (projection, records) = read_back_with_more(SMALL);
    let extended = applied(projection, records);
    assert_eq!(edges_of(extended.state(), "ex-hub"), (2 * SMALL, 0));
    grows_linearly(
        "a projection read back taking as many edges again for one issue, in one batch",
        &[Work::Probe],
        read_back_with_more,
        |(projection, records)| drop(applied(projection, records)),
    );
    grows_linearly(
        "the same, one edge a batch",
        &[Work::Probe],
        read_back_with_more,
        |(projection, records)| drop(one_at_a_time(projection, records)),
    );
}
