//! `sort_for_listing`, the order `bead list` and `bead ready` print in (roll-lqjj.24): priority,
//! then creation time, then key, whatever order the issues are given in, and what it was when it
//! copied each creation time into a string for every comparison.

use beadroll_core::event::Field;
use beadroll_core::json::Value;
use beadroll_core::query::sort_for_listing;
use beadroll_core::state::Issue;
use beadroll_test_support::json;

use super::replay_audit_properties::fixture::{Fixture, fields};

/// The order the sort had before it compared borrowed text: owned creation times, compared whole.
fn reference(items: &mut [&Issue]) {
    let created = |issue: &Issue| {
        issue
            .get(Field::CreatedAt)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    items.sort_by(|a, b| {
        (a.priority().cmp(&b.priority()))
            .then_with(|| created(a).cmp(&created(b)))
            .then_with(|| a.key().cmp(b.key()))
    });
}

fn keys(items: &[&Issue]) -> Vec<String> {
    items.iter().map(|issue| issue.key().to_owned()).collect()
}

#[test]
fn issues_are_listed_by_priority_then_creation_then_key_in_whatever_order_they_come() {
    let mut f = Fixture::new();
    // keys that sort against the order they were made in, three priorities, a boolean one among
    // them, and several made close together
    for (n, priority) in [2, 0, 1, 2, 0, 3, 1, 2, 0, 1, 2, 4].into_iter().enumerate() {
        let uid = f.create(&format!("au-{}", 20 - n));
        let last = f.last();
        f.event(
            "set",
            Some(&uid),
            "seed",
            &[&last],
            fields(Field::Priority, json!(priority)),
        );
    }
    let st = f.state();
    let all: Vec<&Issue> = st.issues().collect();
    let mut wanted = all.clone();
    reference(&mut wanted);
    // the priorities lead, so the order is not the order made or the order of the keys
    assert_ne!(keys(&wanted), keys(&all));
    assert_eq!(
        wanted.first().map(|i| i.priority().to_string()).as_deref(),
        Some("0")
    );
    for rotation in 0..all.len() {
        for reversed in [false, true] {
            let mut given = all.clone();
            given.rotate_left(rotation);
            if reversed {
                given.reverse();
            }
            sort_for_listing(&mut given);
            assert_eq!(
                keys(&given),
                keys(&wanted),
                "rotated by {rotation}, reversed: {reversed}"
            );
        }
    }
}
