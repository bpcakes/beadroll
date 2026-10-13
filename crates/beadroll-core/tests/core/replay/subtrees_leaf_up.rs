//! `State::subtrees_leaf_up`, the order `bead close --recursive` writes in: each root and every
//! issue beneath it once, each after everything beneath it, children in the order they were
//! created, through loops and at any depth.

use beadroll_core::id::EventId;
use beadroll_test_support::{Json, json};

use super::replay_audit_properties::fixture::Fixture;

/// Makes `child` a child of `parent` after the last event.
fn adopt(f: &mut Fixture, child: &str, parent: &str) {
    let last = f.last();
    f.event(
        "dep_add",
        Some(child),
        "seed",
        &[&last],
        json!({"to": parent, "type": "parent-child"}),
    );
}

fn ids(uids: &[&str]) -> Vec<EventId> {
    uids.iter()
        .map(|uid| EventId::parse(uid).unwrap())
        .collect()
}

#[test]
fn every_issue_comes_after_those_beneath_it_and_each_once() {
    let mut f = Fixture::new();
    let root = f.create("au-root");
    let first = f.create("au-first");
    let second = f.create("au-second");
    let deep = f.create("au-deep");
    let other = f.create("au-other");
    // created first, adopted last: the order of creation decides, not of adoption
    adopt(&mut f, &second, &root);
    adopt(&mut f, &first, &root);
    adopt(&mut f, &deep, &first);
    // reached twice: beneath the root through first, and named again
    adopt(&mut f, &deep, &other);
    let st = f.state();
    let order = st.subtrees_leaf_up(&ids(&[&root, &first, &other]));
    assert_eq!(order, ids(&[&deep, &first, &second, &root, &other]));
    // a root with nothing beneath it is itself
    assert_eq!(st.subtrees_leaf_up(&ids(&[&second])), ids(&[&second]));
}

#[test]
fn siblings_come_in_the_order_they_were_made_not_of_their_ids() {
    let mut f = Fixture::new();
    let root = f.create("au-root");
    let first = f.create("au-first");
    let made = f.create("au-second");
    // made after the first, which it saw, under an id that sorts before every other, as two
    // ids of one millisecond can
    let second =
        EventId::from_u128((1_790_000_000_000_u128 << 80) | (7 << 76) | (2 << 62)).to_string();
    f.edit(&made, |event| {
        event.insert("id".into(), Json::from(second.as_str()));
        event.insert("issue".into(), Json::from(second.as_str()));
    });
    adopt(&mut f, &second, &root);
    adopt(&mut f, &first, &root);
    let st = f.state();
    assert_eq!(
        st.subtrees_leaf_up(&ids(&[&root])),
        ids(&[&first, &second, &root])
    );
}

#[test]
fn a_loop_ends_where_it_leads_back() {
    let mut f = Fixture::new();
    let k = f.create("au-k");
    let m = f.create("au-m");
    let z = f.create("au-z");
    adopt(&mut f, &m, &k);
    adopt(&mut f, &k, &m);
    adopt(&mut f, &z, &m);
    let st = f.state();
    assert_eq!(st.subtrees_leaf_up(&ids(&[&k])), ids(&[&z, &m, &k]));
    assert_eq!(st.subtrees_leaf_up(&ids(&[&m])), ids(&[&k, &z, &m]));
}

#[test]
fn a_chain_far_deeper_than_a_thread_stack_takes_is_walked() {
    let mut f = Fixture::new();
    let mut chain = vec![f.create("au-0")];
    for level in 1..20_000 {
        let child = f.create(&format!("au-{level}"));
        adopt(&mut f, &child, &chain[level - 1]);
        chain.push(child);
    }
    let st = f.state();
    let order = st.subtrees_leaf_up(&ids(&[&chain[0]]));
    let expected: Vec<&str> = chain.iter().rev().map(String::as_str).collect();
    assert_eq!(order, ids(&expected));
}
