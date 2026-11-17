//! Taking back what waits for a key: a removal by key or by target, before or beside the create
//! that makes the key, and a compaction before it arrives, each leaves what replay leaves.

use super::super::hostile::{HERE, fields};
use super::{
    CHAINED, THERE, at, base, by_key, by_target, creating, edge, edges, every_way, on, stamp,
    waiting,
};

use crate::json::{Object, Value};

#[test]
fn a_removal_by_key_takes_back_what_waits_of_its_type_and_then_the_edge_it_became() {
    let x = creating(
        "pr-x",
        vec![
            on("pr-k", Some("blocks"), &[]),
            on("pr-k", Some("related"), &[]),
            on("pr-l", Some("blocks"), &[]),
            on("pr-k", Some("blocks"), &[("metadata", "m")]),
        ],
    );
    let rest = vec![
        at(3, HERE, &[2], "create", 3, x),
        at(
            4,
            HERE,
            &[3],
            "dep_remove",
            3,
            by_key("pr-k", Some("blocks")),
        ),
        at(5, HERE, &[4], "create", 5, creating("pr-k", vec![])),
        at(6, HERE, &[5], "dep_remove", 3, by_key("pr-l", None)),
        at(
            7,
            HERE,
            &[6],
            "dep_remove",
            3,
            by_key("pr-k", Some("related")),
        ),
    ];
    let (state, taken) = every_way(&base(), &rest[..3]);
    assert_eq!(taken, CHAINED);
    assert_eq!(edges(&state, 3), [edge("pr-k", "related", stamp(3, &[]))]);
    assert_eq!(waiting(&state, 3), [on("pr-l", Some("blocks"), &[])]);
    let (state, taken) = every_way(&base(), &rest);
    assert_eq!(taken, CHAINED);
    assert!(edges(&state, 3).is_empty());
    assert!(waiting(&state, 3).is_empty());
}

#[test]
fn a_removal_by_target_before_the_target_exists_takes_back_only_an_import_it_had_seen() {
    let x = || {
        creating(
            "pr-x",
            vec![
                on("pr-k", Some("blocks"), &[]),
                on("pr-k", Some("related"), &[]),
            ],
        )
    };
    // It names the create to come, of its type: it had seen the import, so that edge never forms.
    let rest = vec![
        at(3, HERE, &[2], "create", 3, x()),
        at(4, HERE, &[3], "dep_remove", 3, by_target(5, Some("blocks"))),
        at(5, HERE, &[4], "create", 5, creating("pr-k", vec![])),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert_eq!(taken, CHAINED);
    assert_eq!(edges(&state, 3), [edge("pr-k", "related", stamp(3, &[]))]);
    assert!(waiting(&state, 3).is_empty());
    // Of any type, by a machine that had not seen the import: both edges form.
    let rest = vec![
        at(3, HERE, &[2], "create", 3, x()),
        at(4, THERE, &[2], "dep_remove", 3, by_target(5, None)),
        at(5, HERE, &[3], "create", 5, creating("pr-k", vec![])),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert!(taken.in_one_batch);
    assert_eq!(
        edges(&state, 3),
        [
            edge("pr-k", "blocks", stamp(3, &[])),
            edge("pr-k", "related", stamp(3, &[])),
        ]
    );
}

/// What `named` keeps of an issue that its registers no longer name: a knowledge
/// compacted between an import and its key's arrival, with the frontier past the import, still
/// knows its create, whose writer the edge is stamped with (roll-lqjj.43). Only the track compacted
/// at every write-back meets it.
#[test]
fn a_compaction_before_the_key_arrives_keeps_the_create_of_an_import_no_register_names() {
    // the create's title and status written since: no register names it
    let mut status = Object::new();
    status.insert("status".into(), Value::from("in_progress"));
    let mut set_status = Object::new();
    set_status.insert("fields".into(), Value::from(status));
    let x = creating("pr-x", vec![on("pr-k", Some("blocks"), &[])]);
    let rest = vec![
        at(3, HERE, &[2], "create", 3, x),
        at(4, HERE, &[3], "set", 3, fields("retitled")),
        at(5, HERE, &[4], "set", 3, set_status),
        at(6, HERE, &[5], "create", 6, creating("pr-k", vec![])),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert_eq!(taken, CHAINED);
    assert_eq!(edges(&state, 3), [edge("pr-k", "blocks", stamp(3, &[]))]);
}

/// As `a_removal_by_target_before_the_target_exists_takes_back_only_an_import_it_had_seen`, with a
/// write elsewhere between the removal and the target's create: a knowledge compacted before the
/// create has the removal off its frontier, and still knows it from what the issue has taken back
/// (roll-lqjj.43).
#[test]
fn a_compaction_before_the_target_exists_keeps_a_removal_off_the_frontier() {
    let x = creating(
        "pr-x",
        vec![
            on("pr-k", Some("blocks"), &[]),
            on("pr-k", Some("related"), &[]),
        ],
    );
    let rest = vec![
        at(3, HERE, &[2], "create", 3, x),
        at(4, HERE, &[3], "dep_remove", 3, by_target(6, Some("blocks"))),
        at(5, HERE, &[4], "set", 2, fields("elsewhere")),
        at(6, HERE, &[5], "create", 6, creating("pr-k", vec![])),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert_eq!(taken, CHAINED);
    assert_eq!(edges(&state, 3), [edge("pr-k", "related", stamp(3, &[]))]);
    assert!(waiting(&state, 3).is_empty());
}

#[test]
fn a_removal_by_key_concurrent_with_the_targets_create_wins_whichever_applies_first() {
    let x = creating("pr-x", vec![on("pr-b", Some("blocks"), &[])]);
    for (removal, create) in [(4, 5), (5, 4)] {
        let mut rest = vec![
            at(3, HERE, &[2], "create", 3, x.clone()),
            at(removal, HERE, &[3], "dep_remove", 3, by_key("pr-b", None)),
            at(
                create,
                THERE,
                &[3],
                "create",
                create,
                creating("pr-b", vec![]),
            ),
        ];
        rest.sort_by_key(|record| {
            record
                .value
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        let (state, taken) = every_way(&base(), &rest);
        assert!(taken.in_one_batch, "both build on the import");
        assert!(
            edges(&state, 3).is_empty(),
            "removal {removal}, create {create}"
        );
        assert!(waiting(&state, 3).is_empty());
    }
}
