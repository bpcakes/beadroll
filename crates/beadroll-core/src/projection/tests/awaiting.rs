//! Imports that wait for a key (roll-lqjj.18): every way a state is made agrees on what becomes of
//! them — replay of the whole journal, a projection extended a record at a time (read back from its
//! bytes between records, whose index of what waits is then built again from the issues' lists, or
//! never read back, whose index is kept), and one extended by everything in one batch. What each
//! scenario expects, and the digest of the seeded histories, are what replay made of them before
//! the index, at cdf311f: the index changes the work and nothing it makes.

use super::super::{Extended, Record, project};
use super::hostile::{HERE, event, id, journal, record};
use super::shown::replayed;
use super::{TextFile, WriteBack, answers_alike, edges, read_back, same, whole};
use crate::id::EventId;
use crate::journal::JournalOrder;
use crate::json::{Format, Object, Value, write};
use crate::replay::replay;
use crate::state::{Rename, State, asked};
use crate::time::event_time;

mod histories;
mod removals;

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
        // the copy gave way to the first `pr-k.1`, made at 5
        cause: EventId::parse(&id(5)).unwrap(),
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
        let replayed = replayed(&held);
        for state in [projection.state(), &replayed] {
            let issue = state.issue(uid).expect("the waiting issue");
            assert!(
                issue
                    .dangling()
                    .iter()
                    .all(|dep| !matches!(dep, Value::Null)),
                "finished batch after {key} must expose no temporary holes"
            );
            assert_eq!(issue.dangling().len(), listed.len());
        }
        assert_eq!(
            projection.state().awaiting.numbered(),
            listed,
            "after {key}"
        );
        assert!(same(projection.state(), &replayed));
    }
    assert!(waiting(projection.state(), 3).is_empty());
    assert_eq!(edges(projection.state(), 3).len(), 4);
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
        // what a rename answers; its cause is how replay judges a later child, which the states'
        // equality compares, and leaving it out keeps the pinned digests of the answers
        let crate::state::Rename {
            from,
            to,
            replica,
            uid,
            cause: _,
        } = rename;
        out.push_str(&format!(
            "Rename {{ from: {from:?}, to: {to:?}, replica: {replica:?}, uid: {uid:?} }}\n"
        ));
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
