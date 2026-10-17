//! When a projection's knowledge is compacted (roll-lqjj.43): a projection replay makes knows of
//! what every event had seen the events its state names and the frontier alone; written back, it
//! knows every event taken in since as well, until those are more than an eighth of the events it
//! knew when it was last compacted, and the write-back that finds them so compacts it again. Read
//! back on either side, it answers as replay.

use crate::json::{Object, Value};

use super::super::{COMPACTED, Extended, Projection, Record, Unreadable, project};
use super::hostile::{HERE, event, fields, journal, record};
use super::shown::replayed;
use super::{TextFile, WriteBack, answers_alike, read_back, same, whole};

/// An `init`, then issues 2 to `last` created one after another by this machine.
fn created(last: u128) -> Vec<Record> {
    let mut records = vec![record(
        event(1, HERE, &[], "init", 1, Object::new()),
        HERE,
        true,
    )];
    for n in 2..=last {
        let mut create = fields("first");
        create.insert("key".into(), Value::from(format!("pr-{n}")));
        records.push(record(
            event(n, HERE, &[n - 1], "create", n, create),
            HERE,
            true,
        ));
    }
    records
}

/// Event `n` retitles issue 2, after event `n - 1`: the title before it is named no more.
fn retitle(n: u128) -> Record {
    record(
        event(n, HERE, &[n - 1], "set", 2, fields(&format!("title {n}"))),
        HERE,
        true,
    )
}

#[test]
fn a_write_back_compacts_the_knowledge_once_it_holds_more_than_an_eighth_more() {
    // eighty creates: the projection knows them alone, the last the frontier
    let mut records = created(81);
    let mut projection = project(journal(&records)).unwrap();
    assert_eq!((projection.known(), projection.compacted()), (80, 80));
    assert!(!projection.compaction_due());
    let mut file = TextFile::new();
    let (mut compacted, mut since, mut compactions) = (80, 0, 0);
    for n in 82..140 {
        // each retitle leaves the title before it named no more
        records.push(retitle(n));
        let Extended::Applied(extended) = projection.extend(vec![retitle(n)]).unwrap() else {
            panic!("a write on the frontier is taken");
        };
        since += 1;
        assert_eq!(extended.known(), compacted + since, "event {n}");
        // more than an eighth more: an eighth exactly is written as it is
        let due = since * COMPACTED > compacted;
        assert_eq!(extended.compaction_due(), due, "event {n}");
        let read = read_back(&extended, &mut file, WriteBack::Encoded);
        if due {
            // the eighty creates and the latest title
            assert_eq!((read.known(), read.compacted()), (81, 81), "event {n}");
            (compacted, since, compactions) = (81, 0, compactions + 1);
        } else {
            assert_eq!(read.known(), compacted + since, "event {n}");
            assert_eq!(read.compacted(), compacted, "event {n}");
        }
        let replayed = replayed(&records);
        assert!(same(whole(&read, &file).state(), &replayed), "event {n}");
        assert!(answers_alike(&read, &replayed), "event {n}");
        projection = read;
    }
    // at 80: ten retitles written as they were, the eleventh compacted; at 81 the same
    assert_eq!(compactions, 5);
}

#[test]
fn a_projection_of_no_events_is_compacted_by_the_first_event_taken_in() {
    let projection = project(journal(&[])).unwrap();
    assert_eq!((projection.known(), projection.compacted()), (0, 0));
    assert!(!projection.compaction_due());
    let records = created(2);
    let mut taken = project(journal(&records[..1])).unwrap();
    assert_eq!((taken.known(), taken.compacted()), (1, 1)); // the init, the frontier
    let Extended::Applied(extended) = taken.extend(records[1..].to_vec()).unwrap() else {
        panic!("a write on the frontier is taken");
    };
    taken = *extended;
    // one event more than the one known: more than an eighth
    assert!(taken.compaction_due());
    let read = read_back(&taken, &mut TextFile::new(), WriteBack::Encoded);
    assert_eq!((read.known(), read.compacted()), (1, 1)); // the create, the frontier
}

#[test]
fn bytes_that_say_more_events_were_compacted_than_are_known_never_read_as_a_projection() {
    let mut projection = project(journal(&created(9))).unwrap();
    projection.compacted = projection.known() + 1;
    let (encoded, _) = projection.encode_with(0, false);
    assert_eq!(
        Projection::decode(&encoded).unwrap_err(),
        Unreadable("the events last compacted")
    );
    projection.compacted = projection.known();
    let (encoded, _) = projection.encode_with(0, false);
    assert!(Projection::decode(&encoded).is_ok());
}
