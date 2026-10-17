//! What a write-back copies of the bytes a projection was read back from (roll-lqjj.44), in the
//! cases the differential histories do not draw: ids taken in that sort before and among those
//! held, which a machine's clock gone back mints, and a state part's table that every write-back
//! adds to. [`super::read_back`] checks each write-back here as it checks the histories': what it
//! copied is what it writes when it copies nothing, byte for byte, and it reads back as written.

use crate::json::{Object, Value};

use super::super::parts::TABLE_SLACK;
use super::super::{Extended, Projection, Record, project};
use super::hostile::{HERE, event, fields, journal, record};
use super::{TextFile, WriteBack, equal, read_back};

/// An `init` and issue 101 made by `HERE` as events 100 and 101, then titles set on it as the even
/// events 102 to 120, each after the one before, ids ascending.
fn held() -> Vec<Record> {
    let mut create = fields("first");
    create.insert("key".into(), Value::from("pr-a1"));
    let mut records = vec![
        record(
            event(100, HERE, &[], "init", 100, Object::new()),
            HERE,
            true,
        ),
        record(event(101, HERE, &[100], "create", 101, create), HERE, true),
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
        records.push(record(set, HERE, true));
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
            record(set, HERE, true)
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
        let set = record(set, HERE, true);
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
