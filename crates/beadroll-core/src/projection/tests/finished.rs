//! What a projection derives once a batch has applied (roll-lqjj.17): a projection extended one
//! event at a time keeps each issue's labels and the parent-child index as replay of everything so
//! far leaves them, though it finishes only what the batch can have changed. A close changes
//! neither, which is what a bulk close relies on; a label operation changes the labels of the
//! issue it names; a dependency written, or a create that gives an import that waited for its key
//! an edge, changes the index. A batch derives each issue's labels from what it holds once every
//! one of its events has applied, however many of them change that issue.

use super::super::{Extended, Projection, Record, project};
use super::hostile::{HERE, event, id, journal, record};
use super::same;
use super::shown::replayed;
use crate::id::EventId;
use crate::journal::JournalOrder;
use crate::json::{Object, Value};
use crate::text::str_of;

/// `rest` applied to a projection of `base` one event at a time, as a command that writes one
/// event per issue and reads the state between them does: each time, what the projection derives
/// is what replay of everything applied so far derives.
fn one_at_a_time(base: &[Record], rest: &[Record]) {
    let mut projection = project(journal(base)).unwrap();
    let mut held: Vec<Record> = base.to_vec();
    for (n, next) in rest.iter().enumerate() {
        held.push(next.clone());
        projection = match projection.extend(vec![next.clone()]).unwrap() {
            Extended::Applied(extended) => *extended,
            Extended::Replay => panic!("event {} is one a projection applies", n + 1),
        };
        let full = replayed(&held);
        assert!(
            same(projection.state(), &full),
            "after event {}: {:?} against {:?}",
            n + 1,
            projection.state().issues().collect::<Vec<_>>(),
            full.issues().collect::<Vec<_>>(),
        );
    }
}

/// `rest` applied to a projection of `base` as one batch, which is what a store takes a write or a
/// merge in as: what the projection derives is what replay of everything derives.
fn in_one_batch(base: &[Record], rest: &[Record]) -> Projection {
    let projection = project(journal(base)).unwrap();
    let extended = match projection.extend(rest.to_vec()).unwrap() {
        Extended::Applied(extended) => *extended,
        Extended::Replay => panic!("the batch is one a projection applies"),
    };
    let mut held: Vec<Record> = base.to_vec();
    held.extend_from_slice(rest);
    let full = replayed(&held);
    assert!(same(extended.state(), &full));
    extended
}

/// The labels of the issue numbered `n`, as the state derived them.
fn labels_of(projection: &Projection, n: u128) -> Vec<String> {
    let uid = EventId::parse(&id(n)).unwrap();
    (projection.state().issue(uid).unwrap().labels().iter())
        .map(String::clone)
        .collect()
}

/// A create of the issue numbered `n` under `key`, with `labels` and the imported dependencies
/// `dangling` on keys named as text.
fn creating(key: &str, labels: &[&str], dangling: &[(&str, &str)]) -> Object {
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from(key));
    let mut data = Object::new();
    data.insert("key".into(), Value::from(key));
    data.insert("fields".into(), Value::from(fields));
    if !labels.is_empty() {
        let labels = labels.iter().map(|label| Value::from(*label)).collect();
        data.insert("labels".into(), Value::Array(labels));
    }
    if !dangling.is_empty() {
        let deps = (dangling.iter())
            .map(|(to, kind)| {
                let mut dep = Object::new();
                dep.insert("depends_on_id".into(), Value::from(*to));
                dep.insert("type".into(), Value::from(*kind));
                Value::from(dep)
            })
            .collect();
        data.insert("dangling".into(), Value::Array(deps));
        data.insert("imported".into(), Value::from(true));
    }
    data
}

/// The labels `labels` name.
fn labels(labels: &[&str]) -> Object {
    let mut data = Object::new();
    let labels = labels.iter().map(|label| Value::from(*label)).collect();
    data.insert("labels".into(), Value::Array(labels));
    data
}

/// A dependency on the issue numbered `to`, of `kind`.
fn edge(to: u128, kind: &str) -> Object {
    let mut data = Object::new();
    data.insert("to".into(), Value::from(id(to)));
    data.insert("type".into(), Value::from(kind));
    data
}

/// An `init` and two issues, each with a label: the frontier is event 3.
fn base() -> Vec<Record> {
    vec![
        record(
            event(1, HERE, &[], "init", 1, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(2, HERE, &[1], "create", 2, creating("pr-a1", &["ui"], &[])),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(3, HERE, &[2], "create", 3, creating("pr-b2", &["ui"], &[])),
            HERE,
            JournalOrder::Sequenced,
        ),
    ]
}

#[test]
fn a_close_one_event_at_a_time_leaves_the_labels_and_the_index_as_replay_does() {
    let base = base();
    let closes = vec![
        record(
            event(4, HERE, &[3], "close", 2, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(5, HERE, &[4], "close", 3, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(6, HERE, &[5], "reopen", 2, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(7, HERE, &[6], "delete", 3, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
    ];
    one_at_a_time(&base, &closes);
}

#[test]
fn every_label_operation_one_event_at_a_time_leaves_the_labels_as_replay_does() {
    let base = base();
    let written = vec![
        // added, taken away, and set, which takes away every label the writer saw
        record(
            event(4, HERE, &[3], "label_add", 2, labels(&["db", "ops"])),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(5, HERE, &[4], "label_remove", 2, labels(&["ui"])),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(6, HERE, &[5], "label_set", 2, labels(&["only"])),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(7, HERE, &[6], "label_set", 3, labels(&[])),
            HERE,
            JournalOrder::Sequenced,
        ),
        // a create carries the labels an import brought
        record(
            event(8, HERE, &[7], "create", 8, creating("pr-c3", &["new"], &[])),
            HERE,
            JournalOrder::Sequenced,
        ),
    ];
    one_at_a_time(&base, &written);
}

#[test]
fn a_batch_that_changes_one_issues_labels_over_and_over_derives_them_from_what_it_holds() {
    // Four events change issue 2's labels and one changes issue 3's. The labels are those the
    // issues hold once all five have applied, derived once for each issue and not once for each
    // event (roll-lqjj.17, Z02 follow-up).
    let base = base();
    let churn = vec![
        record(
            event(4, HERE, &[3], "label_add", 2, labels(&["db"])),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(5, HERE, &[4], "label_add", 2, labels(&["ops"])),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(6, HERE, &[5], "label_remove", 2, labels(&["ui"])),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(7, HERE, &[6], "label_set", 2, labels(&["final"])),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(8, HERE, &[7], "label_add", 3, labels(&["x"])),
            HERE,
            JournalOrder::Sequenced,
        ),
    ];
    let batched = in_one_batch(&base, &churn);
    assert_eq!(
        labels_of(&batched, 2),
        ["final"],
        "the set the last one gives"
    );
    assert_eq!(labels_of(&batched, 3), ["ui", "x"]);
    // The same events one at a time reach the same labels, as replay does either way.
    one_at_a_time(&base, &churn);
}

#[test]
fn every_dependency_written_one_event_at_a_time_leaves_the_index_as_replay_does() {
    let base = base();
    let written = vec![
        record(
            event(4, HERE, &[3], "dep_add", 3, edge(2, "parent-child")),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(5, HERE, &[4], "dep_add", 3, edge(2, "blocks")),
            HERE,
            JournalOrder::Sequenced,
        ),
        // taken back by target and type, which leaves the edge with no addition behind it
        record(
            event(6, HERE, &[5], "dep_remove", 3, edge(2, "parent-child")),
            HERE,
            JournalOrder::Sequenced,
        ),
        // a create whose imported dependency is on a key that has its issue already
        record(
            event(
                7,
                HERE,
                &[6],
                "create",
                7,
                creating("pr-c3", &[], &[("pr-a1", "parent-child")]),
            ),
            HERE,
            JournalOrder::Sequenced,
        ),
    ];
    one_at_a_time(&base, &written);
}

#[test]
fn a_create_that_gives_a_waiting_import_its_edge_leaves_the_index_as_replay_does() {
    // The import named a key no issue had, so the edge is the created issue's, given by a create
    // that is not about it: the whole index is built again whenever an edge can have moved.
    let mut base = base();
    base.push(record(
        event(
            4,
            HERE,
            &[3],
            "create",
            4,
            creating("pr-c3", &[], &[("pr-d4", "parent-child")]),
        ),
        HERE,
        JournalOrder::Sequenced,
    ));
    let waiting = project(journal(&base)).unwrap();
    let waits = EventId::parse(&id(4)).unwrap();
    assert!(
        waiting.state().issue(waits).unwrap().deps().is_empty(),
        "no issue has the key the import names yet"
    );
    let written = vec![record(
        event(5, HERE, &[4], "create", 5, creating("pr-d4", &[], &[])),
        HERE,
        JournalOrder::Sequenced,
    )];
    one_at_a_time(&base, &written);
}

/// A projection of `base` with `rest` applied one event at a time.
fn extended(base: &[Record], rest: Vec<Record>) -> Projection {
    let mut projection = project(journal(base)).unwrap();
    for next in rest {
        projection = match projection.extend(vec![next]).unwrap() {
            Extended::Applied(extended) => *extended,
            Extended::Replay => panic!("a projection applies these"),
        };
    }
    projection
}

#[test]
fn the_ids_a_projection_holds_stay_ascending_however_the_events_sort() {
    let base = base();
    // The second event comes before the first by id, as two writes of one millisecond can.
    let out_of_order = vec![
        record(
            event(9, HERE, &[3], "close", 2, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(4, HERE, &[9], "close", 3, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
    ];
    let projection = extended(&base, out_of_order.clone());
    let mut every: Vec<String> = (base.iter().chain(&out_of_order))
        .filter_map(|r| r.value.get("id").map(str_of))
        .collect();
    every.sort();
    let held: Vec<String> = (projection.ids.iter()).map(EventId::to_string).collect();
    assert_eq!(held, every, "every id once, ascending");
    let mut held = base;
    held.extend(out_of_order);
    assert!(same(projection.state(), &replayed(&held)));
}

#[test]
fn ids_that_interleave_with_the_ones_held_go_in_where_they_belong_in_one_batch() {
    // What a machine writes while its clock is behind the ids it holds, or a merge of a machine
    // whose clock ran ahead: the new ids are not all after the held ones, and none of them is
    // appended (roll-lqjj.17, Z02 follow-up). The bytes still hold them ascending, which the
    // decoder requires, so the set cannot hide a wrong order.
    let base = base();
    let interleaved = vec![
        record(
            event(9, HERE, &[3], "close", 2, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(4, HERE, &[9], "close", 3, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
    ];
    let batched = in_one_batch(&base, &interleaved);
    let held: Vec<String> = (batched.ids.iter()).map(EventId::to_string).collect();
    let mut every: Vec<String> = (base.iter().chain(&interleaved))
        .filter_map(|r| r.value.get("id").map(str_of))
        .collect();
    every.sort();
    assert_eq!(held, every, "every id once, ascending");
    let (encoded, _) = batched.encode(0);
    let read_back = Projection::decode(encoded).unwrap();
    assert_eq!(read_back.ids, batched.ids, "the bytes keep them ascending");
}

#[test]
fn each_machines_latest_event_is_its_greatest_id_however_its_records_sort() {
    // What `bead machines` says a machine last wrote (roll-lqjj.31): its greatest id, which its
    // last record need not have when its clock went back. Projected, extended one event at a
    // time, in one batch, and read back from the bytes, a projection says what the whole
    // journal does.
    let base = base();
    let out_of_order = vec![
        record(
            event(9, HERE, &[3], "close", 2, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(4, HERE, &[9], "close", 3, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
    ];
    let mut held = base.clone();
    held.extend(out_of_order.clone());
    let nine = EventId::parse(&id(9));
    let full = project(journal(&held)).unwrap();
    assert_eq!(full.journals(), [(HERE, 5, nine)]);
    assert_eq!(full.event_count(), 5);
    let one_by_one = extended(&base, out_of_order.clone());
    let batched = in_one_batch(&base, &out_of_order);
    for projection in [&one_by_one, &batched] {
        assert_eq!(projection.journals(), full.journals());
        assert_eq!(projection.event_count(), full.event_count());
        let (encoded, _) = projection.encode(0);
        let read_back = Projection::decode(encoded.clone()).unwrap();
        assert_eq!(read_back.journals(), full.journals());
        assert_eq!(read_back.event_count(), full.event_count());
    }
}
