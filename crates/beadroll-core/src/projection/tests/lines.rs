//! A batch read from the lines a store's journal files grew by is the batch of the records those
//! lines are: a projection reads the same issues' texts for it, and extends by it as it extends by
//! the records built in memory, or declines both alike.

use super::super::{Appended, Extended, Projection, Record, journal_of, project};
use super::{HistoryKind, history};
use crate::json::{Format, write};

/// How many records each batch after a projection holds.
const BATCH: usize = 3;

/// `records` as a store's files hold them, read as a store reads what they grew by: each run of
/// records of one file the lines of that file, written as bead writes them.
fn read_from_lines(records: &[Record]) -> Appended {
    let mut appended = Appended::new();
    let mut at = 0;
    while let Some(first) = records.get(at) {
        let run = (records[at..].iter())
            .take_while(|record| record.origin == first.origin)
            .count();
        let mut data = Vec::new();
        for record in &records[at..at + run] {
            data.extend(write(&record.value, Format::COMPACT).unwrap().into_bytes());
            data.push(b'\n');
        }
        if first.sequenced {
            appended.add_segment(&data, &first.replica, &first.origin);
        } else {
            appended.add_attempt(&data, &first.replica, &first.origin);
        }
        at += run;
    }
    appended
}

/// The bytes of `projection`, encoded as a store writes it.
fn bytes(projection: &Projection) -> [Vec<u8>; 3] {
    let (encoded, _) = projection.encode(0);
    [encoded.state, encoded.index, encoded.texts]
}

/// Each history held up to every place but its last few, and the next records after it applied to
/// a projection of what is held, read both ways: how many extensions applied.
fn extended_alike(kind: HistoryKind) -> usize {
    let mut applied = 0;
    for seed in 0..16 {
        let records = history(20_261_126 + seed, 8, kind);
        for held in 1..records.len().saturating_sub(BATCH) {
            let Ok(projection) = project(journal_of(records[..held].iter().cloned())) else {
                continue;
            };
            let batch = &records[held..held + BATCH];
            let lines = read_from_lines(batch);
            let built: Appended = batch.iter().cloned().collect();
            assert_eq!(
                projection.about(&lines),
                projection.about(&built),
                "seed {seed}, held {held}"
            );
            let by_lines = projection.clone().extend(lines).unwrap();
            let by_records = projection.extend(built).unwrap();
            match (by_lines, by_records) {
                (Extended::Applied(lines), Extended::Applied(records)) => {
                    assert_eq!(bytes(&lines), bytes(&records), "seed {seed}, held {held}");
                    applied += 1;
                }
                (Extended::Replay, Extended::Replay) => {}
                (lines, records) => panic!(
                    "seed {seed}, held {held}: from lines {}, from records {}",
                    matches!(lines, Extended::Applied(_)),
                    matches!(records, Extended::Applied(_)),
                ),
            }
        }
    }
    applied
}

#[test]
fn a_batch_read_from_lines_extends_a_projection_as_its_records_do() {
    assert!(extended_alike(HistoryKind::Valid) > 0, "nothing applied");
}

#[test]
fn a_flawed_batch_read_from_lines_is_declined_or_applied_as_its_records_are() {
    assert!(extended_alike(HistoryKind::Flawed) > 0, "nothing applied");
}

#[test]
fn an_unreadable_line_leaves_the_batch_unreadable() {
    let mut appended = Appended::new();
    appended.add_segment(b"{\"v\":1}\nnot json\n{}\n", "a", "segment");
    assert!(!appended.is_readable());
    let mut appended = Appended::new();
    appended.add_segment(b"", "a", "segment");
    assert!(appended.is_readable() && appended.is_empty());
}
