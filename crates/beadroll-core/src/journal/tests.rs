//! A journal reads a line straight into what it keeps of a record, and a record built in memory
//! as the line bead writes of it (roll-lqjj.53.1): both ways, the journal is the same, and the
//! files are read by the rules the value readers keep.

use std::collections::BTreeSet;

use super::*;
use crate::journal::JournalOrder;
use crate::json::parse;
use crate::projection::Record;
use crate::projection::tests::HistoryKind;
use crate::projection::tests::history;

/// Event `n` by machine `a`, as one line without its newline.
fn line(n: u64, op: &str, issue: Option<u64>, data: &str) -> String {
    let id = id(n);
    let issue = issue.map_or_else(|| id.clone(), id_of);
    format!(
        r#"{{"v":1,"id":"{id}","replica":"a","deps":[],"actor":{{"agent":"t"}},"op":"{op}","issue":"{issue}","data":{data}}}"#
    )
}

/// The id of event `n`.
fn id(n: u64) -> String {
    format!("0190{:04x}-0000-7000-8000-{:012x}", n % 0x1_0000, n)
}

fn id_of(n: u64) -> String {
    id(n)
}

/// A segment of `lines`, each terminated.
fn segment(lines: &[&str]) -> Vec<u8> {
    lines
        .iter()
        .flat_map(|l| format!("{l}\n").into_bytes())
        .collect()
}

fn problems(journal: &Journal) -> Vec<(String, String)> {
    (journal.invalid.iter())
        .map(|p| (p.origin.clone(), p.problem.clone()))
        .collect()
}

/// `records`, as a store's files hold them (one file per origin, in the order first found), read
/// into a journal as values and as lines: both settled, taken apart, and compared whole.
fn both_ways(records: &[Record]) -> Parts {
    let origins: Vec<&str> = {
        let mut seen = BTreeSet::new();
        (records.iter().map(|r| r.origin.as_str()))
            .filter(|origin| seen.insert(*origin))
            .collect()
    };
    // Two journals with one set of digest keys, so that their events' digests compare.
    let mut values = Journal::new();
    let mut lines = values.clone();
    for origin in origins {
        let of_file: Vec<&Record> = records.iter().filter(|r| r.origin == origin).collect();
        let replica = &of_file[0].replica;
        let sequenced = of_file[0].sequenced;
        let mut text = Vec::new();
        for record in &of_file {
            assert_eq!(record.sequenced, sequenced);
            values.add(
                record.value.clone(),
                replica,
                origin,
                if sequenced {
                    JournalOrder::Sequenced
                } else {
                    JournalOrder::Attempt
                },
            );
            text.extend(
                json::write(&record.value, Format::COMPACT)
                    .unwrap()
                    .into_bytes(),
            );
            text.push(b'\n');
        }
        if sequenced {
            lines.add_segment(&text, replica, origin);
        } else {
            lines.add_attempt(&text, replica, origin);
        }
    }
    assert_eq!(values.heads(), lines.heads());
    let (values, lines) = (values.into_parts(), lines.into_parts());
    assert!(values.events.iter().eq(lines.events.iter()));
    assert_eq!(values.order, lines.order);
    assert_eq!(values.quarantined, lines.quarantined);
    assert_eq!(values.invalid, lines.invalid);
    lines
}

#[test]
fn a_journal_read_from_lines_is_the_journal_of_the_values_read_from_them() {
    for seed in 0..6 {
        let parts = both_ways(&history(20_261_022 + seed, 12, HistoryKind::Valid));
        assert!(parts.events.len() > 12, "seed {seed}");
        assert!(parts.invalid.is_empty(), "seed {seed}");
    }
}

#[test]
fn a_journal_of_what_replay_reports_reads_alike_either_way() {
    let mut reported = 0;
    for seed in 0..6 {
        let parts = both_ways(&history(20_261_022 + seed, 12, HistoryKind::Flawed));
        reported += parts.invalid.len() + parts.quarantined.len();
    }
    assert!(reported > 0, "the flawed histories report nothing");
}

#[test]
fn a_segment_skips_a_last_line_not_yet_whole_and_reports_any_other() {
    let mut data = segment(&[
        &line(1, "init", None, r#"{"prefix":"ex"}"#),
        r#"{"v":1,"id":"#,
        &line(
            2,
            "create",
            None,
            r#"{"key":"ex-1","fields":{"title":"t"}}"#,
        ),
    ]);
    data.extend_from_slice(br#"{"v":1,"id":"0190"#);
    let mut journal = Journal::new();
    journal.add_segment(&data, "a", "events/a/000001.jsonl");
    assert_eq!(
        problems(&journal),
        [(
            "events/a/000001.jsonl:2".to_owned(),
            "line is not valid JSON".to_owned()
        )]
    );
    assert_eq!(journal.events().len(), 2);
    assert_eq!(journal.order()["a"].len(), 2);
    // as the value reader reports the same bytes
    let mut reported = Vec::new();
    let values = read_segment(&data, "events/a/000001.jsonl", Some(&mut reported));
    assert_eq!(values.len(), 2);
    assert_eq!(reported, journal.invalid);
}

#[test]
fn a_segment_line_that_is_no_event_is_reported_with_its_file() {
    let data = segment(&[
        "[1, 2]",
        r#"{"id": "x", "op": "init"}"#,
        &line(1, "init", None, "{}"),
    ]);
    let mut journal = Journal::new();
    journal.add_segment(&data, "a", "events/a/000001.jsonl");
    assert_eq!(
        problems(&journal),
        [
            (
                "events/a/000001.jsonl".to_owned(),
                "event is not an object".to_owned()
            ),
            (
                "events/a/000001.jsonl".to_owned(),
                "id is not a UUID".to_owned()
            ),
        ]
    );
    assert_eq!(journal.events().len(), 1);
}

#[test]
fn an_attempt_is_read_whole_or_not_at_all() {
    let good = segment(&[
        &line(1, "init", None, r#"{"prefix":"ex"}"#),
        &line(
            2,
            "create",
            None,
            r#"{"key":"ex-1","fields":{"title":"t"}}"#,
        ),
    ]);
    let mut journal = Journal::new();
    journal.add_attempt(&good, "a", "events/a/claims/x.json");
    journal.settle();
    assert!(journal.invalid.is_empty());
    assert_eq!(journal.events().len(), 2);
    assert!(
        journal.order().is_empty(),
        "an attempt's events have no place in the order"
    );

    let cases = [
        (
            segment(&[&line(1, "init", None, "{}"), "{\"v\":"]),
            "line is not valid JSON",
        ),
        (
            segment(&[&line(1, "init", None, "{}"), "[]"]),
            "attempt line is not an event",
        ),
        (
            segment(&[
                &line(1, "init", None, "{}"),
                r#"{"id":"x","op":"frobnicate","data":null}"#,
            ]),
            "id is not a UUID",
        ),
        (
            segment(&[&line(1, "init", None, "{}"), &line(2, "create", None, "{}")]),
            "create event has no valid 'key'",
        ),
        (
            segment(&[
                &line(1, "init", None, "{}"),
                &line(2, "frobnicate", None, "{}"),
            ]),
            "unknown operation 'frobnicate'",
        ),
    ];
    for (data, problem) in cases {
        let mut journal = Journal::new();
        journal.add_attempt(&data, "a", "events/a/claims/x.json");
        assert_eq!(
            problems(&journal),
            [(
                "events/a/claims/x.json:2".to_owned(),
                format!("{problem}; the whole attempt is ignored")
            )]
        );
        assert!(journal.events().is_empty(), "{problem}");
        let mut reported = Vec::new();
        assert!(read_attempt(&data, "events/a/claims/x.json", Some(&mut reported)).is_empty());
        assert_eq!(reported, journal.invalid, "{problem}");
    }
}

#[test]
fn an_attempt_keeps_members_it_does_not_validate() {
    let mut value = parse(line(1, "init", None, r#"{"prefix":"ex"}"#).as_bytes())
        .unwrap()
        .value;
    let Value::Object(object) = &mut value else {
        panic!("an event is an object");
    };
    object.insert(
        "future".to_owned(),
        parse(br#"{"nested":[{"text":"kept","count":3}]}"#)
            .unwrap()
            .value,
    );
    let bytes = json::write(&value, Format::COMPACT).unwrap();
    let mut reported = Vec::new();
    assert_eq!(
        read_attempt(bytes.as_bytes(), "attempt", Some(&mut reported)),
        vec![value]
    );
    assert!(reported.is_empty());
}

#[test]
fn a_line_is_one_content_whatever_the_whitespace_around_it() {
    let text = line(1, "init", None, r#"{"prefix":"ex"}"#);
    let mut journal = Journal::new();
    journal.add_segment(format!("{text}\n").as_bytes(), "a", "events/a/000001.jsonl");
    journal.add_segment(
        format!("  {text}\r\n").as_bytes(),
        "a",
        "events/a/000002.jsonl",
    );
    journal.add(
        parse(text.as_bytes()).unwrap().value,
        "a",
        "events/a/000003.jsonl",
        JournalOrder::Sequenced,
    );
    journal.settle();
    assert!(journal.quarantined().is_empty());
    assert_eq!(journal.events().len(), 1);
    assert!(journal.invalid.is_empty());
}

#[test]
fn a_copy_written_otherwise_is_another_content() {
    // The same members in another order, or spaced: bead never writes a record twice, so two
    // lines of one content are the same bytes, and a line that differs is another content.
    let text = line(1, "init", None, r#"{"prefix":"ex"}"#);
    let spaced = text.replace("\"v\":1,", "\"v\": 1, ");
    assert_eq!(
        parse(text.as_bytes()).unwrap(),
        parse(spaced.as_bytes()).unwrap()
    );
    let mut journal = Journal::new();
    journal.add_segment(format!("{text}\n").as_bytes(), "a", "events/a/000001.jsonl");
    journal.add_segment(
        format!("{spaced}\n").as_bytes(),
        "a",
        "events/a/000002.jsonl",
    );
    journal.settle();
    assert_eq!(journal.quarantined().len(), 1);
    assert_eq!(journal.events().len(), 1);
    assert_eq!(journal.events()[0].op_name(), "quarantined");
}
