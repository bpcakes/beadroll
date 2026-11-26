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

/// Bytes a segment grew by that end in a line not yet whole: the batch is unreadable, and a
/// projection declines it, rather than applying it without that line as a journal skips it.
#[test]
fn a_tail_ending_in_a_line_not_yet_whole_is_declined() {
    let records = history(20_261_126, 8, HistoryKind::Valid);
    let (held, next) = records.split_at(records.len() - 1);
    let projection = project(journal_of(held.iter().cloned())).unwrap();
    let line = write(&next[0].value, Format::COMPACT).unwrap();
    let whole = read_from_lines(next);
    assert!(whole.is_readable());
    for cut in [1, line.len() / 2, line.len()] {
        let mut appended = Appended::new();
        appended.add_segment(&line.as_bytes()[..cut], &next[0].replica, &next[0].origin);
        assert!(!appended.is_readable(), "cut at {cut}");
        assert!(matches!(
            projection
                .clone()
                .prepare_appended(appended)
                .apply()
                .unwrap(),
            Extended::Replay
        ));
    }
}

/// A malformed JSON line alone rules out extension (AU05).
#[test]
fn an_unreadable_line_leaves_the_batch_unreadable() {
    let mut appended = Appended::new();
    appended.add_segment(b"not json\n", "a", "segment");
    assert!(!appended.is_readable());
    let mut appended = Appended::new();
    appended.add_segment(b"", "a", "segment");
    assert!(appended.is_readable() && appended.is_empty());
}

/// A missing newline rules out the entire segment before parsing. Later additions to
/// the same rejected batch also do no work (AU25).
#[test]
fn an_incomplete_segment_stops_the_batch_before_parsing_its_prefix() {
    use super::hostile::base;
    use crate::work::{Work, counted};

    let record = base().remove(0);
    let mut line = write(&record.value, Format::COMPACT).unwrap().into_bytes();
    line.push(b'\n');
    let mut partial = line.clone();
    partial.push(b'{');
    let mut batch = Appended::new();
    let (_, work) = counted(|| batch.add_segment(&line, &record.replica, &record.origin));
    assert!(batch.is_readable());
    assert_eq!(work.of(Work::JournalLine), 1);
    let replica =
        std::sync::Arc::downgrade(batch.readable().unwrap().records()[0].shared_replica());

    let (_, work) = counted(|| {
        batch.add_segment(&partial, &record.replica, &record.origin);
        batch.add_segment(&line, &record.replica, &record.origin);
        batch.add_attempt(&line, &record.replica, &record.origin);
        batch.add(record);
    });
    assert!(!batch.is_readable());
    assert!(
        replica.upgrade().is_none(),
        "the batch kept its earlier records"
    );
    assert_eq!(work.of(Work::JournalLine), 0);
    assert_eq!(work.of(Work::RecordDigest), 0);
}

/// Catch-up stops at the first unreadable record. Full replay reports it and recovers
/// the later event from the same segment (AU26).
#[test]
fn catch_up_stops_at_a_bad_line_that_full_replay_reads_past() {
    use super::hostile::base;
    use crate::journal::Journal;
    use crate::work::{Work, counted};

    let records = base();
    let first = &records[0];
    let before = write(&first.value, Format::COMPACT).unwrap();
    let after = write(&records[1].value, Format::COMPACT).unwrap();
    for broken in ["not json", "{}"] {
        let data = format!("{before}\n \t\n{broken}\n{after}\n");
        let mut batch = Appended::new();
        let (_, work) =
            counted(|| batch.add_segment(data.as_bytes(), &first.replica, &first.origin));
        assert!(!batch.is_readable());
        assert_eq!(work.of(Work::JournalLine), 2, "{broken}");

        let mut full = Journal::new();
        full.add_segment(data.as_bytes(), &first.replica, &first.origin);
        let settled = full.settled_view();
        assert_eq!(settled.invalid().len(), 1);
        let ops: Vec<_> = settled
            .events()
            .values()
            .map(|event| event.op_name())
            .collect();
        assert_eq!(ops, ["init", "create"]);
    }
}

/// A public caller with raw records pays no serialization cost when the projection is already
/// committed to replay. An empty batch still leaves that projection alone (AU08).
#[test]
fn preparation_checks_for_replay_before_hashing_in_memory_records() {
    use super::hostile::{HERE, base, event, fields, record};
    use crate::journal::JournalOrder;
    use crate::work::{Work, counted};

    let next = |deps: &[u128]| {
        record(
            event(4, HERE, deps, "set", 2, fields("next")),
            HERE,
            JournalOrder::Sequenced,
        )
    };
    let open = project(journal_of(base())).unwrap();
    let mut held = base();
    held.push(next(&[99]));
    let waiting = project(journal_of(held)).unwrap();
    assert!(!waiting.extendable());

    let prepare = |projection: Projection, records: Vec<Record>| {
        counted(|| {
            let extension = projection.prepare_extension(records);
            assert!(extension.wanted_texts().is_empty());
            extension.apply().unwrap()
        })
    };
    let (result, work) = prepare(waiting.clone(), vec![next(&[3])]);
    assert!(matches!(result, Extended::Replay));
    assert_eq!(work.of(Work::RecordDigest), 0);
    let (result, _) = prepare(waiting, Vec::new());
    assert!(matches!(result, Extended::Applied(p) if !p.extendable()));
    let (result, work) = prepare(open, vec![next(&[3])]);
    assert!(matches!(result, Extended::Applied(_)));
    assert_eq!(work.of(Work::RecordDigest), 1);
}

/// A shared input keeps its original operations. Both the extended state and a full replay
/// retain only the event metadata, including for a superseded update (AU21).
#[test]
fn applying_a_shared_batch_releases_its_operations_from_the_state() {
    use super::hostile::{HERE, base, event, fields, record};
    use crate::journal::{Event, JournalOrder};
    use crate::replay::replay;

    let updates: Vec<Record> = [(4, "superseded"), (5, "current")]
        .into_iter()
        .map(|(n, title)| {
            record(
                event(n, HERE, &[n - 1], "set", 2, fields(title)),
                HERE,
                JournalOrder::Sequenced,
            )
        })
        .collect();
    let batch = Appended::from(updates.clone());
    let mut journal = journal_of(base().into_iter().chain(updates));
    let replayed = replay(journal.clone()).unwrap();
    let start = project(journal_of(base())).unwrap();
    let Extended::Applied(projected) = start
        .clone()
        .prepare_appended(batch.clone())
        .apply()
        .unwrap()
    else {
        panic!("a sequential batch should extend the projection");
    };
    for original in batch.readable().unwrap().records() {
        let id = original.id();
        let metadata = Event::applied(
            id,
            original.deps().into(),
            original.shared_replica().clone(),
            original.shared_actor().cloned(),
        );
        assert_eq!(original.op_name(), "set");
        assert_eq!(journal.settled_view().event(id).unwrap().op_name(), "set");
        for state in [projected.state(), &replayed] {
            assert!(
                state.knowledge.events.get(&id) == Some(&metadata),
                "retained the operation for {id}"
            );
        }
    }
    let Extended::Applied(again) = start.prepare_appended(batch).apply().unwrap() else {
        panic!("the original batch must still apply");
    };
    assert_eq!(bytes(&again), bytes(&projected));
}
