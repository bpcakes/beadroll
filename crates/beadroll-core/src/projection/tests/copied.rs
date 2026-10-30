//! What a write-back copies of the bytes a projection was read back from (roll-lqjj.44), in the
//! cases the differential histories do not draw: ids taken in that sort before and among those
//! held, which a machine's clock gone back mints, and a state part's table that every write-back
//! adds to, made anew while issues it does not change are there to be copied. [`super::read_back`] checks each write-back here as it checks the histories': what it
//! copied is what it writes when it copies nothing, byte for byte, and it reads back as written.

use super::super::parts::TABLE_SLACK;
use super::super::{Extended, Projection, Record, project};
use super::hostile::{HERE, event, fields, journal, record};
use super::{TextFile, WriteBack, equal, read_back};
use crate::journal::JournalOrder;
use crate::json::{Object, Value};

/// An `init` and issue 101 made by `HERE` as events 100 and 101, then titles set on it as the even
/// events 102 to 120, each after the one before, ids ascending.
fn held() -> Vec<Record> {
    let mut create = fields("first");
    create.insert("key".into(), Value::from("pr-a1"));
    let mut records = vec![
        record(
            event(100, HERE, &[], "init", 100, Object::new()),
            HERE,
            JournalOrder::Sequenced,
        ),
        record(
            event(101, HERE, &[100], "create", 101, create),
            HERE,
            JournalOrder::Sequenced,
        ),
    ];
    for n in (102..=120).step_by(2) {
        let before = if n == 102 { 101 } else { n - 2 };
        let set = event(
            n,
            HERE,
            &[before],
            "set",
            101,
            fields(&format!("title {n}")),
        );
        records.push(record(set, HERE, JournalOrder::Sequenced));
    }
    records
}

/// `projection`, read back and written back from `file`, extended by `batch` as a store extends it.
fn taken(projection: &Projection, file: &mut TextFile, batch: Vec<Record>) -> Projection {
    let mut read = read_back(projection, file, WriteBack::Uncompacted);
    let about = read.about(&batch);
    super::read_in(read.state_mut(), file, Some(about));
    match read.extend(batch).unwrap() {
        Extended::Applied(extended) => *extended,
        Extended::Replay => panic!("the batch is declined"),
    }
}

#[test]
fn ids_taken_in_before_and_among_those_held_are_merged_where_they_sort() {
    let mut records = held();
    let first = project(journal(&records)).unwrap();
    // the next three after the frontier, minted by a clock gone back: before every id held, among
    // them, and after them
    let batch: Vec<Record> = [(50, 120), (111, 50), (200, 111)]
        .into_iter()
        .map(|(n, after)| {
            let set = event(n, HERE, &[after], "set", 101, fields(&format!("again {n}")));
            record(set, HERE, JournalOrder::Sequenced)
        })
        .collect();
    records.extend(batch.iter().cloned());
    let mut file = TextFile::new();
    let extended = taken(&first, &mut file, batch);
    assert_eq!(extended.added.len(), 3);
    let read = read_back(&extended, &mut file, WriteBack::Uncompacted);
    let full = project(journal(&records)).unwrap();
    assert!(equal(&super::whole(&read, &file), &full));
    assert_eq!(read.ids, full.ids);
}

#[test]
fn a_state_part_table_is_made_anew_once_it_holds_an_eighth_more_entries_and_a_few() {
    let mut records = held();
    let mut projection = project(journal(&records)).unwrap();
    let mut file = TextFile::new();
    let (mut most, mut made_anew) = (0, 0);
    // each write-back writes the issue again, at a place its table has not held: the place before
    // is held by nothing any more, and stays in the table until it is made anew
    for n in 121..=520 {
        let mut set = event(n, HERE, &[n - 1], "set", 101, fields("placed"));
        let mut place = Object::new();
        place.insert("branch".into(), Value::from(format!("branch {n}")));
        if let Value::Object(members) = &mut set {
            members.insert("where".into(), Value::from(place));
        }
        let set = record(set, HERE, JournalOrder::Sequenced);
        records.push(set.clone());
        projection = taken(&projection, &mut file, vec![set]);
        let kept = projection.kept.as_deref().unwrap();
        let entries = kept.state_table.names.len() + kept.state_table.values.len();
        assert!(
            entries <= kept.fresh + kept.fresh / 8 + TABLE_SLACK,
            "{entries} entries"
        );
        most = most.max(entries);
        made_anew += usize::from(entries <= kept.fresh + 1);
    }
    assert!(most > TABLE_SLACK, "the table grew to {most} entries");
    assert!(made_anew >= 4, "made anew {made_anew} times");
    let full = project(journal(&records)).unwrap();
    let read = read_back(&projection, &mut file, WriteBack::Uncompacted);
    assert!(equal(&super::whole(&read, &file), &full));
}

/// `value`, an event, made on the branch `branch`.
fn placed_on(mut value: Value, branch: &str) -> Value {
    let mut place = Object::new();
    place.insert("branch".into(), Value::from(branch));
    if let Value::Object(members) = &mut value {
        members.insert("where".into(), Value::from(place));
    }
    value
}

/// A state part's table made anew holds only what the records then written name, so every record
/// is written again, those of the issues that did not change too: copied, theirs would name entries
/// of the table before by number, and read back as others. Issue 101 is placed on a branch once and
/// never changed; issue 103 is moved to a branch of its own at every write-back, until the table is
/// made anew several times, and [`read_back`] compares each write-back with one that copies nothing
/// (found by the review of roll-lqjj.44: copying there passed every other test).
#[test]
fn a_table_made_anew_has_every_record_written_again_those_unchanged_too() {
    let mut still = fields("still");
    still.insert("key".into(), Value::from("pr-still"));
    let mut moving = fields("moving");
    moving.insert("key".into(), Value::from("pr-moving"));
    let init = event(100, HERE, &[], "init", 100, Object::new());
    let still = placed_on(event(101, HERE, &[100], "create", 101, still), "home");
    let moving = event(103, HERE, &[101], "create", 103, moving);
    let mut records = vec![
        record(init, HERE, JournalOrder::Sequenced),
        record(still, HERE, JournalOrder::Sequenced),
        record(moving, HERE, JournalOrder::Sequenced),
    ];
    let mut projection = project(journal(&records)).unwrap();
    let mut file = TextFile::new();
    let mut made_anew = 0;
    for n in 104..=500 {
        let set = event(n, HERE, &[n - 1], "set", 103, fields("moved"));
        let set = record(
            placed_on(set, &format!("b{n}")),
            HERE,
            JournalOrder::Sequenced,
        );
        records.push(set.clone());
        projection = taken(&projection, &mut file, vec![set]);
        let kept = projection.kept.as_deref().unwrap();
        let entries = kept.state_table.names.len() + kept.state_table.values.len();
        made_anew += usize::from(entries <= kept.fresh + 1);
    }
    assert!(made_anew >= 3, "made anew {made_anew} times");
    let full = project(journal(&records)).unwrap();
    let read = read_back(&projection, &mut file, WriteBack::Uncompacted);
    assert!(equal(&super::whole(&read, &file), &full));
}
