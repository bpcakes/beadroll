//! Imports that wait for a key (roll-lqjj.18): every way a state is made agrees on what becomes of
//! them — replay of the whole journal, a projection extended a record at a time (read back from its
//! bytes between records, whose index of what waits is then built again from the issues' lists, or
//! never read back, whose index is kept), and one extended by everything in one batch. What each
//! scenario expects, and the digest of the seeded histories, are what replay made of them before
//! the index, at cdf311f: the index changes the work and nothing it makes.

use super::super::{Extended, Record, project};
use super::hostile::{HERE, event, fields, id, journal, record};
use super::shown::replayed;
use super::{
    MACHINES, TextFile, World, WriteBack, answers_alike, edges, equal, load, read_back, read_in,
    records, same, whole,
};
use crate::id::EventId;
use crate::journal::JournalOrder;
use crate::json::{Format, Object, Value, write};
use crate::projection::tests::edges::AnswerCoverage;
use crate::replay::replay;
use crate::state::{Rename, State, asked};
use crate::time::event_time;

/// Another machine.
pub(super) const THERE: &str = "0000bb02";

/// An imported dependency on `key`, of `kind` when given, with the members `extra` adds.
pub(super) fn on(key: &str, kind: Option<&str>, extra: &[(&str, &str)]) -> Value {
    let mut dep = Object::new();
    dep.insert("depends_on_id".into(), Value::from(key));
    if let Some(kind) = kind {
        dep.insert("type".into(), Value::from(kind));
    }
    for (name, value) in extra {
        dep.insert((*name).into(), Value::from(*value));
    }
    Value::from(dep)
}

/// The create of an issue under `key`, imported, with the dependencies `named`.
pub(super) fn creating(key: &str, named: Vec<Value>) -> Object {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(key));
    let mut data = Object::new();
    data.insert("key".into(), Value::from(key));
    data.insert("fields".into(), Value::from(fields));
    data.insert("imported".into(), Value::from(true));
    if !named.is_empty() {
        data.insert("dangling".into(), Value::Array(named));
    }
    data
}

/// A removal of what an issue's import named of `key`, of `kind` or of any type.
pub(super) fn by_key(key: &str, kind: Option<&str>) -> Object {
    let mut data = Object::new();
    data.insert("key".into(), Value::from(key));
    data.insert("type".into(), kind.map_or(Value::Null, Value::from));
    data
}

/// A removal of the edges to the issue numbered `to`, of `kind` or of any type.
fn by_target(to: u128, kind: Option<&str>) -> Object {
    let mut data = Object::new();
    data.insert("to".into(), Value::from(id(to)));
    data.insert("type".into(), kind.map_or(Value::Null, Value::from));
    data
}

/// Event `n` of `replica`, built on `deps`, as a record of that machine's journal.
pub(super) fn at(
    n: u128,
    replica: &str,
    deps: &[u128],
    op: &str,
    issue: u128,
    data: Object,
) -> Record {
    record(
        event(n, replica, deps, op, issue, data),
        replica,
        JournalOrder::Sequenced,
    )
}

/// What every scenario starts from: an `init`, and an issue the other machine made after it, so that
/// a projection knows both machines' journals. Its records build on event 2.
pub(super) fn base() -> Vec<Record> {
    let mut data = Object::new();
    data.insert("prefix".into(), Value::from("pr"));
    vec![
        at(1, HERE, &[], "init", 1, data),
        at(2, THERE, &[1], "create", 2, creating("pr-base", vec![])),
    ]
}

/// The metadata an edge an import of issue `n` brought is stamped with: its create's time and
/// writer, then what the import carried.
fn stamp(n: u128, extra: &[(&str, &str)]) -> Value {
    let uid = EventId::parse(&id(n)).unwrap();
    let mut meta = Object::new();
    meta.insert("created_at".into(), Value::from(event_time(uid.millis())));
    meta.insert("created_by".into(), Value::from(format!("human@{HERE}")));
    for (name, value) in extra {
        meta.insert((*name).into(), Value::from(*value));
    }
    Value::from(meta)
}

/// The issue numbered `n`.
fn issue(state: &State, n: u128) -> &crate::state::Issue {
    state.issue(EventId::parse(&id(n)).unwrap()).unwrap()
}

/// Issue `n`'s edges, in their order: the key of each target, its type, and its metadata.
fn edges(state: &State, n: u128) -> Vec<(String, String, Value)> {
    (issue(state, n).deps().iter())
        .map(|(edge, meta)| {
            let to = state.target_key(edge.to()).into_owned();
            (to, edge.kind().to_owned(), edges::shown(meta))
        })
        .collect()
}

/// Issue `n`'s imported dependencies still waiting for their keys, in their order.
fn waiting(state: &State, n: u128) -> Vec<Value> {
    issue(state, n).dangling().to_vec()
}

/// An edge to `key` of `kind`, with `meta`.
fn edge(key: &str, kind: &str, meta: Value) -> (String, String, Value) {
    (key.to_owned(), kind.to_owned(), meta)
}

/// How a scenario's records were taken in besides replay.
#[derive(Debug, PartialEq)]
pub(super) struct Taken {
    /// Whether a projection of `base` took them all in one batch.
    pub(super) in_one_batch: bool,
    /// How many a projection taking them one at a time declined, replaying instead.
    pub(super) declined: usize,
}

/// What replay makes of `base` and then `rest`, once every way a projection can take `rest` in
/// agrees with replay of what it holds after each record: one at a time, never read back and read
/// back from its bytes before each, and in one batch. A projection declines a record it cannot
/// apply after everything it holds, as a merge of concurrent events is, and is then made again.
pub(super) fn every_way(base: &[Record], rest: &[Record]) -> (State, Taken) {
    let mut all = base.to_vec();
    all.extend_from_slice(rest);
    let full = replayed(&all);
    let in_one_batch = match project(journal(base)).unwrap().extend(rest.to_vec()) {
        Ok(Extended::Applied(extended)) => {
            assert!(same(extended.state(), &full), "in one batch");
            true
        }
        Ok(Extended::Replay) => false,
        Err(error) => panic!("{error}"),
    };
    let mut declined = 0;
    // never read back, and read back each way it can be written back (roll-lqjj.43)
    for reading_back in [None].into_iter().chain(WriteBack::EVERY.map(Some)) {
        let (mut file, mut held) = (TextFile::new(), base.to_vec());
        let mut projection = project(journal(base)).unwrap();
        for (n, next) in rest.iter().enumerate() {
            held.push(next.clone());
            if let Some(back) = reading_back {
                projection = read_back(&projection, &mut file, back);
            }
            projection = match projection.extend(vec![next.clone()]).unwrap() {
                Extended::Applied(extended) => *extended,
                Extended::Replay => {
                    declined += usize::from(reading_back.is_none());
                    project(journal(&held)).unwrap()
                }
            };
            let replayed = replayed(&held);
            assert!(
                same(whole(&projection, &file).state(), &replayed)
                    && answers_alike(&projection, &replayed),
                "after record {} (read back: {reading_back:?})",
                n + 1
            );
        }
        // which holds the bytes to the same state
        read_back(
            &projection,
            &mut file,
            reading_back.unwrap_or(WriteBack::Encoded),
        );
    }
    let taken = Taken {
        in_one_batch,
        declined,
    };
    (full, taken)
}

/// One machine's records, each built on the one before it, the first on `base`'s `init`: what a
/// projection takes in one at a time.
pub(super) const CHAINED: Taken = Taken {
    in_one_batch: true,
    declined: 0,
};

#[test]
fn imports_naming_keys_made_later_get_their_edges_as_each_arrives() {
    let x = creating(
        "pr-x",
        vec![
            on(
                "pr-a",
                Some("blocks"),
                &[("created_by", "alice"), ("metadata", "m1")],
            ),
            on("pr-b", Some("related"), &[]),
            on("pr-c", None, &[]),
        ],
    );
    let gone = creating("pr-zz", vec![on("pr-gone", Some("blocks"), &[])]);
    let rest = vec![
        at(3, HERE, &[2], "create", 3, x),
        at(4, HERE, &[3], "create", 4, creating("pr-b", vec![])),
        // a key nobody waits for, from an import that waits for one that never comes
        at(5, HERE, &[4], "create", 5, gone),
        at(6, HERE, &[5], "create", 6, creating("pr-a", vec![])),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert_eq!(taken, CHAINED);
    // in the order the keys came, the last entry, whose key never came, still waiting
    let imported = stamp(3, &[("created_by", "alice"), ("metadata", "m1")]);
    assert_eq!(
        edges(&state, 3),
        [
            edge("pr-b", "related", stamp(3, &[])),
            edge("pr-a", "blocks", imported),
        ]
    );
    assert_eq!(waiting(&state, 3), [on("pr-c", None, &[])]);
    assert_eq!(waiting(&state, 5), [on("pr-gone", Some("blocks"), &[])]);
}

#[test]
fn a_key_named_again_and_in_several_types_gives_each_type_one_edge_and_waits_for_nothing_else() {
    let x = creating(
        "pr-x",
        vec![
            on("pr-k", Some("blocks"), &[("metadata", "first")]),
            on("pr-k", Some("related"), &[]),
            on("pr-k", Some("blocks"), &[("metadata", "second")]),
            on("pr-l", Some("blocks"), &[]),
            on("pr-k", Some("parent-child"), &[]),
        ],
    );
    let y = creating("pr-y", vec![on("pr-k", Some(""), &[])]);
    let rest = vec![
        at(3, HERE, &[2], "create", 3, x),
        at(4, HERE, &[3], "create", 4, y),
        at(5, HERE, &[4], "create", 5, creating("pr-k", vec![])),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert_eq!(taken, CHAINED);
    // the first entry of a type gives its metadata; a falsy type is a block
    assert_eq!(
        edges(&state, 3),
        [
            edge("pr-k", "blocks", stamp(3, &[("metadata", "first")])),
            edge("pr-k", "related", stamp(3, &[])),
            edge("pr-k", "parent-child", stamp(3, &[])),
        ]
    );
    assert_eq!(waiting(&state, 3), [on("pr-l", Some("blocks"), &[])]);
    assert_eq!(edges(&state, 4), [edge("pr-k", "blocks", stamp(4, &[]))]);
    assert!(waiting(&state, 4).is_empty());
    let k = EventId::parse(&id(5)).unwrap();
    assert_eq!(state.children(k), [EventId::parse(&id(3)).unwrap()]);
}

#[test]
fn what_an_import_names_of_its_own_key_waits_until_it_is_taken_back() {
    let y = creating("pr-y", vec![on("pr-s", Some("related"), &[])]);
    let x = creating(
        "pr-s",
        vec![
            on("pr-s", Some("blocks"), &[]),
            on("pr-t", Some("blocks"), &[]),
        ],
    );
    let rest = vec![
        at(3, HERE, &[2], "create", 3, y),
        at(4, HERE, &[3], "create", 4, x),
        at(5, HERE, &[4], "create", 5, creating("pr-t", vec![])),
        at(6, HERE, &[5], "dep_remove", 4, by_key("pr-s", None)),
    ];
    let (state, taken) = every_way(&base(), &rest[..3]);
    assert_eq!(taken, CHAINED);
    // Its own key settles the other issue's entry and not its own.
    assert_eq!(edges(&state, 3), [edge("pr-s", "related", stamp(3, &[]))]);
    assert_eq!(edges(&state, 4), [edge("pr-t", "blocks", stamp(4, &[]))]);
    assert_eq!(waiting(&state, 4), [on("pr-s", Some("blocks"), &[])]);
    let (state, taken) = every_way(&base(), &rest);
    assert_eq!(taken, CHAINED);
    assert!(waiting(&state, 4).is_empty(), "taken back by its key");
    assert_eq!(edges(&state, 4), [edge("pr-t", "blocks", stamp(4, &[]))]);
}

#[test]
fn a_copy_renamed_for_a_key_two_machines_made_takes_a_key_nothing_waits_for() {
    let v = creating(
        "pr-v",
        vec![
            on("pr-k.1", Some("blocks"), &[]),
            on("pr-k.2", Some("blocks"), &[]),
            on("pr-k.3", Some("related"), &[]),
        ],
    );
    // The other machine's copy of pr-k.1 applies after this one's: it gives way under the first
    // key that is neither an issue's nor waited for.
    let copy = at(6, THERE, &[2], "create", 6, creating("pr-k.1", vec![]));
    let rest = vec![
        at(3, HERE, &[2], "create", 3, v.clone()),
        at(5, HERE, &[3], "create", 5, creating("pr-k.1", vec![])),
        copy.clone(),
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert!(taken.in_one_batch);
    let renamed = |to: &str| Rename {
        from: "pr-k.1".into(),
        to: to.into(),
        replica: THERE.into(),
        uid: EventId::parse(&id(6)).unwrap(),
    };
    assert_eq!(state.renames(), [renamed("pr-k.4")]);
    assert_eq!(edges(&state, 3), [edge("pr-k.1", "blocks", stamp(3, &[]))]);
    assert_eq!(
        waiting(&state, 3),
        [
            on("pr-k.2", Some("blocks"), &[]),
            on("pr-k.3", Some("related"), &[])
        ]
    );
    // Taken back in the same batch before the copy comes, pr-k.2 is waited for no longer, so the
    // copy takes it, and no edge comes of it.
    let rest = vec![
        at(3, HERE, &[2], "create", 3, v),
        at(4, HERE, &[3], "dep_remove", 3, by_key("pr-k.2", None)),
        at(5, HERE, &[4], "create", 5, creating("pr-k.1", vec![])),
        copy,
    ];
    let (state, taken) = every_way(&base(), &rest);
    assert!(taken.in_one_batch);
    assert_eq!(state.renames(), [renamed("pr-k.2")]);
    assert_eq!(edges(&state, 3), [edge("pr-k.1", "blocks", stamp(3, &[]))]);
    assert_eq!(waiting(&state, 3), [on("pr-k.3", Some("related"), &[])]);
}

/// The asks of [`State::claims_key`] made while `make` makes a state, each as its key, whether it is
/// claimed, by a dependency that waits alone, and while an entry taken out holds its place. Every
/// ask answered as collecting every key would, or the hook that keeps them fails.
pub(super) fn asks_making(make: impl FnOnce() -> State) -> Vec<(String, bool, bool, bool)> {
    asked::taken();
    make();
    (asked::taken().into_iter())
        .map(|ask| (ask.key, ask.claimed, ask.awaited, ask.mid_batch))
        .collect()
}

#[test]
fn a_renamed_copy_asks_of_each_key_it_tries_whether_an_issue_has_it_or_an_import_waits_for_it() {
    let v = creating(
        "pr-v",
        vec![
            on("pr-k.1", Some("blocks"), &[]),
            on("pr-k.2", Some("blocks"), &[]),
            on("pr-k.3", Some("related"), &[]),
        ],
    );
    let copy = at(6, THERE, &[2], "create", 6, creating("pr-k.1", vec![]));
    let mine = at(5, HERE, &[4], "create", 5, creating("pr-k.1", vec![]));
    let removal = at(4, HERE, &[3], "dep_remove", 3, by_key("pr-k.2", None));
    let made = |rest: &[Record]| {
        let mut all = base();
        all.extend_from_slice(rest);
        let replayed = asks_making(|| replay(journal(&all)).unwrap());
        let in_one_batch =
            asks_making(
                || match project(journal(&base())).unwrap().extend(rest.to_vec()) {
                    Ok(Extended::Applied(extended)) => extended.state().clone(),
                    _ => panic!("a projection takes them in one batch"),
                },
            );
        assert_eq!(replayed, in_one_batch);
        replayed
    };
    let ask = |key: &str, claimed, awaited| (key.to_owned(), claimed, awaited, true);
    // The copy counts up from its own key. pr-k.1's issue took out the entry that waited for it,
    // and its list is made whole once the batch is applied: the copy asks while it holds its
    // place, and finds the two keys still waited for claimed though no issue has them.
    let creates = at(4, HERE, &[3], "create", 4, creating("pr-k.1", vec![]));
    let rest = [
        at(3, HERE, &[2], "create", 3, v.clone()),
        creates,
        copy.clone(),
    ];
    assert_eq!(
        made(&rest),
        [
            ask("pr-k.1", true, false),
            ask("pr-k.2", true, true),
            ask("pr-k.3", true, true),
            ask("pr-k.4", false, false)
        ]
    );
    // Taken back by key in the same batch, pr-k.2 is no longer waited for: the copy takes it.
    let rest = [
        at(3, HERE, &[2], "create", 3, v),
        removal,
        mine,
        copy.clone(),
    ];
    assert_eq!(
        made(&rest),
        [ask("pr-k.1", true, false), ask("pr-k.2", false, false)]
    );
    // With nothing waiting, an issue's key alone is claimed.
    let rest = [
        at(3, HERE, &[2], "create", 3, creating("pr-k.2", vec![])),
        at(4, HERE, &[3], "create", 4, creating("pr-k.1", vec![])),
        copy,
    ];
    let settled = |key: &str, claimed| (key.to_owned(), claimed, false, false);
    assert_eq!(
        made(&rest),
        [
            settled("pr-k.1", true),
            settled("pr-k.2", true),
            settled("pr-k.3", false)
        ]
    );
}

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

#[test]
fn an_entry_keeps_its_number_however_many_before_it_go_batch_after_batch() {
    // One issue waits for five keys. A removal by key takes the second, and then the others come in
    // the order it names them, one create a batch, each taking the first entry left: every batch
    // makes the list whole again, and the entries left stay listed as they were, never listed
    // again (F1), while each state answers as replay does.
    let keys = ["pr-k1", "pr-k2", "pr-k3", "pr-k4", "pr-k5"];
    let named = (keys.iter())
        .map(|key| on(key, Some("blocks"), &[]))
        .collect();
    let mut held = base();
    held.push(at(3, HERE, &[2], "create", 3, creating("pr-x", named)));
    let mut projection = project(journal(&held)).unwrap();
    let uid = EventId::parse(&id(3)).unwrap();
    let mut listed: Vec<(String, EventId, usize)> = (keys.iter().enumerate())
        .map(|(number, key)| ((*key).to_owned(), uid, number))
        .collect();
    assert_eq!(projection.state().awaiting.numbered(), listed);
    let mut batches = vec![("pr-k2", by_key("pr-k2", None), "dep_remove", 3)];
    for (n, key) in (5..).zip(["pr-k1", "pr-k3", "pr-k4", "pr-k5"]) {
        batches.push((key, creating(key, vec![]), "create", n));
    }
    for (n, (key, data, op, issue)) in (4..).zip(batches) {
        let next = at(n, HERE, &[n - 1], op, issue, data);
        held.push(next.clone());
        projection = match projection.extend(vec![next]).unwrap() {
            Extended::Applied(extended) => *extended,
            Extended::Replay => panic!("a projection applies {key}'s batch"),
        };
        listed.retain(|(named, ..)| named != key);
        assert_eq!(
            projection.state().awaiting.numbered(),
            listed,
            "after {key}"
        );
        assert!(same(projection.state(), &replayed(&held)));
    }
    assert!(waiting(projection.state(), 3).is_empty());
    assert_eq!(edges(projection.state(), 3).len(), 4);
}

/// How many keys the seeded histories create, `pr-k0` onwards, and how many they name: the last
/// five never have an issue.
const MADE: usize = 25;
const NAMED: usize = 30;

/// Key number `n` of the seeded histories.
fn key(n: usize) -> String {
    format!("pr-k{n}")
}

/// The types an import names: the usual three, none, and a falsy one, both of which are blocks.
const KINDS: [Option<&str>; 5] = [
    Some("blocks"),
    Some("related"),
    Some("parent-child"),
    None,
    Some(""),
];

/// One thing machine `m` does in a history heavy in imports that wait: mostly creates, many of them
/// imports naming keys that are not issues yet (their own among them), and removals by key and by
/// target.
fn act(world: &mut World, m: usize) {
    let (issues, _) = world.issues(m);
    let roll = world.rng.below(100);
    let issue = world.rng.pick(&issues).cloned();
    let mut data = Object::new();
    let Some(issue) = issue.filter(|_| roll >= 35) else {
        let made = key(world.rng.below(MADE));
        let mut named = Vec::new();
        if world.rng.chance(70) {
            for n in 0..1 + world.rng.below(4) {
                let target = if world.rng.chance(10) {
                    made.clone()
                } else {
                    key(world.rng.below(NAMED))
                };
                let kind = KINDS[world.rng.below(KINDS.len())];
                let mut extra = Vec::new();
                let tag = format!("m{roll}.{n}");
                if world.rng.chance(30) {
                    extra.push(("metadata", tag.as_str()));
                }
                if world.rng.chance(20) {
                    extra.push(("created_by", "importer"));
                }
                if world.rng.chance(10) {
                    extra.push(("thread_id", "t9"));
                }
                named.push(on(&target, kind, &extra));
            }
        }
        world.write(m, "create", None, creating(&made, named));
        return;
    };
    let kind = |world: &mut World| ["blocks", "related", "parent-child"].get(world.rng.below(4));
    let op = match roll {
        35..=59 => {
            data = by_key(&key(world.rng.below(NAMED)), kind(world).copied());
            "dep_remove"
        }
        60..=74 => {
            let to = world.rng.pick(&issues).cloned().unwrap_or_default();
            data.insert("to".into(), Value::from(to));
            data.insert(
                "type".into(),
                kind(world).map_or(Value::Null, |k| Value::from(*k)),
            );
            "dep_remove"
        }
        75..=84 => {
            let to = world.rng.pick(&issues).cloned().unwrap_or_default();
            data.insert("to".into(), Value::from(to));
            data.insert("type".into(), Value::from("blocks"));
            data.insert("meta".into(), Value::from(Object::new()));
            "dep_add"
        }
        _ => {
            let mut fields = Object::new();
            fields.insert("title".into(), Value::from(format!("t{roll}")));
            data.insert("fields".into(), Value::from(fields));
            "set"
        }
    };
    world.write(m, op, Some(issue), data);
}

/// What machine 0's store takes in next, as [`World::next_batch`] chooses it, with [`act`].
fn next_batch(world: &mut World) -> Vec<usize> {
    loop {
        let m = world.rng.below(MACHINES.len());
        if m == 0 && world.rng.chance(60) {
            let before = world.events.len();
            for _ in 0..1 + world.rng.below(3) {
                act(world, 0);
            }
            return (before..world.events.len()).collect();
        }
        if m == 0 {
            return world.sync(0);
        }
        if world.rng.chance(70) {
            world.sync(m);
        }
        for _ in 0..1 + world.rng.below(3) {
            act(world, m);
        }
        world.sync(m);
    }
}

/// What replay makes of imports that wait, in its order: each issue's key, its edges with their
/// metadata and its entries still waiting, and every rename, as text.
pub(super) fn rendered(state: &State) -> String {
    let text = |value: &Value| write(value, Format::COMPACT).unwrap_or_default();
    let mut out = String::new();
    for issue in state.issues() {
        out.push_str(&format!("{} {}\n", issue.uid(), issue.key()));
        for (edge, meta) in issue.deps().iter() {
            let (to, kind) = (edge.to(), edge.kind());
            out.push_str(&format!("  {to} {kind} {}\n", text(&edges::shown(meta))));
        }
        for dep in issue.dangling() {
            out.push_str(&format!("  waits {}\n", text(dep)));
        }
    }
    for rename in state.renames() {
        out.push_str(&format!("{rename:?}\n"));
    }
    out
}

/// FNV-1a over `bytes`, from `hash`.
pub(crate) fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// What the seeded histories exercise, counted in what replay makes of them.
#[derive(Debug, Default)]
struct Exercised {
    /// Edges an import brought to an issue made after the importing one: it waited for the key.
    waited: usize,
    /// Entries still waiting, and those among them that name their own issue's key.
    waiting: usize,
    own: usize,
    /// Copies renamed, and batches a projection took rather than replaying.
    renames: usize,
    applied: usize,
    /// Keys asked of while copies were renamed, those claimed by a dependency that waits alone, and
    /// those asked while an entry taken out of a list held its place.
    asked: usize,
    awaited: usize,
    mid_batch: usize,
}

impl Exercised {
    fn count(&mut self, state: &State) {
        let places: std::collections::HashMap<EventId, usize> = (state.issues().enumerate())
            .map(|(at, issue)| (issue.uid(), at))
            .collect();
        for issue in state.issues() {
            let created = event_time(issue.uid().millis());
            for (edge, meta) in issue.deps().iter() {
                let meta = edges::shown(meta);
                let imported = meta.get("created_at").and_then(Value::as_str) == Some(&*created)
                    || meta.get("created_by").and_then(Value::as_str) == Some("importer");
                let later = (edge.to().id().and_then(|to| places.get(&to)))
                    .is_some_and(|to| *to > places[&issue.uid()]);
                self.waited += usize::from(imported && later);
            }
            self.waiting += issue.dangling().len();
            self.own += (issue.dangling().iter())
                .filter(|dep| dep.get("depends_on_id").and_then(Value::as_str) == Some(issue.key()))
                .count();
        }
        self.renames += state.renames().len();
    }

    /// Counts the asks made since the last were taken.
    fn asks(&mut self) {
        for ask in asked::taken() {
            self.asked += 1;
            self.awaited += usize::from(ask.awaited);
            self.mid_batch += usize::from(ask.mid_batch);
        }
    }
}

/// Fails unless the finished `state` answers of every key it holds or names, of every key the
/// histories make or name, and of the keys their renames tried, whether it claims it as collecting
/// every key does.
fn claims_as_collected(state: &State) {
    let collected = state.claimed_keys();
    let mut keys: Vec<String> = collected.iter().map(|key| (*key).to_owned()).collect();
    keys.extend((0..NAMED + 5).map(key));
    for rename in state.renames() {
        keys.extend([rename.from.clone(), format!("{}0", rename.from)]);
    }
    for key in &keys {
        let claimed = state.claims_key(key);
        assert_eq!(claimed, collected.contains(key.as_str()), "{key}");
    }
}

/// The digest [`rendered`] gives of every seed's whole history, as replay made it at cdf311f,
/// before the index: replay must make the same of them.
const BEFORE_THE_INDEX: u64 = 0xbffd_7bce_ab55_9b7c;

#[test]
fn histories_heavy_in_imports_that_wait_agree_every_way_and_with_replay_before_the_index() {
    let mut digest = 0xcbf2_9ce4_8422_2325;
    let mut exercised = Exercised::default();
    let mut answers = Vec::new();
    for seed in 0..30 {
        let mut answered = edges::START;
        let mut world = World::new(20_261_015 + seed);
        let mut data = Object::new();
        data.insert("prefix".into(), Value::from("pr"));
        world.write(0, "init", None, data);
        let mut held: Vec<usize> = vec![0];
        let first = project(load(&world, &held)).unwrap();
        // each way it can be written back (roll-lqjj.43), the figures the store's
        let mut tracks = WriteBack::EVERY.map(|back| (first.clone(), TextFile::new(), back));
        for n in 0..50 {
            let batch = next_batch(&mut world);
            let reading_back = world.rng.chance(50);
            held.extend(&batch);
            let full = project(load(&world, &held)).unwrap();
            let replayed = replay(load(&world, &held)).unwrap();
            assert!(same(full.state(), &replayed) && answers_alike(&full, &replayed));
            answered = edges::fold(
                answered,
                full.state(),
                if n == 49 {
                    AnswerCoverage::EveryWay
                } else {
                    AnswerCoverage::Direct
                },
            );
            let records = records(&world, &batch);
            tracks = tracks.map(|(mut projection, mut file, back)| {
                if reading_back {
                    projection = read_back(&projection, &mut file, back);
                }
                let about = projection.about(&records);
                read_in(projection.state_mut(), &file, Some(about));
                let projection = match projection.extend(records.clone()).unwrap() {
                    Extended::Applied(extended) => {
                        assert!(
                            equal(&whole(&extended, &file), &full),
                            "seed {seed}, {back:?}"
                        );
                        exercised.applied += usize::from(back == WriteBack::Encoded);
                        *extended
                    }
                    Extended::Replay => full.clone(),
                };
                (projection, file, back)
            });
        }
        let everything: Vec<usize> = (0..world.events.len()).collect();
        let state = replay(load(&world, &everything)).unwrap();
        exercised.asks();
        claims_as_collected(&state);
        asked::taken();
        digest = fnv(digest, rendered(&state).as_bytes());
        exercised.count(&state);
        answers.push((seed, answered));
    }
    let seen = &exercised;
    assert!(
        seen.waited > 500
            && seen.waiting > 500
            && seen.own > 50
            && seen.renames > 500
            && seen.applied > 500,
        "{exercised:?}"
    );
    // Every rename asked as collecting every key would, in the batch an entry was taken out in and
    // of keys only an import waits for among them (`asked`, which fails otherwise).
    assert!(
        seen.asked > seen.renames && seen.awaited > 100 && seen.mid_batch > 500,
        "{exercised:?}"
    );
    assert_eq!(digest, BEFORE_THE_INDEX, "{digest:#x}");
    edges::pinned("awaiting", &answers);
}
