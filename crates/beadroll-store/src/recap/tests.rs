//! The cursor's save over a cursor another recap saved meanwhile: what each recap did to a tracker
//! is kept, whichever saves last, without what of the other's frontier the later one's read placed,
//! and a tracker neither touched keeps its mark.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use beadroll_core::id::EventId;

use super::{Mark, RecapCursor};
use crate::Settings;

/// The settings of a machine whose home is `home`.
fn settings(home: &Path) -> Settings {
    let mut vars: BTreeMap<OsString, OsString> = BTreeMap::new();
    vars.insert("BEAD_HOME".into(), home.as_os_str().to_owned());
    Settings::from_process(vars, None, OsString::new(), std::env::temp_dir(), &|_| None).unwrap()
}

/// The event `n`, written `n` milliseconds into an afternoon in 2026.
fn id(n: u128) -> EventId {
    EventId::from_u128(((1_790_000_000_000 + n) << 80) | (7 << 76) | (2 << 62) | n)
}

/// A tracker shown up to the events `ns`.
fn shown(ns: &[u128]) -> Mark {
    Mark::Shown(ns.iter().map(|n| id(*n)).collect())
}

fn marks(entries: &[(&str, Mark)]) -> BTreeMap<String, Mark> {
    (entries.iter())
        .map(|(tracker, mark)| ((*tracker).to_owned(), mark.clone()))
        .collect()
}

/// `marks` as a recap saves them, each read having placed no more than its own frontier.
fn placing(marks: BTreeMap<String, Mark>) -> BTreeMap<String, (Mark, Vec<EventId>)> {
    (marks.into_iter())
        .map(|(tracker, mark)| {
            let placed = match &mark {
                Mark::Shown(frontier) => frontier.clone(),
                Mark::Owed(_) => Vec::new(),
            };
            (tracker, (mark, placed))
        })
        .collect()
}

#[test]
fn a_save_keeps_what_a_recap_that_saved_meanwhile_did() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings(home.path());
    let start = marks(&[
        ("both-shown", shown(&[1])),
        ("owed-then-shown", Mark::Owed(100)),
        ("only-first", shown(&[1])),
        ("both-owed", Mark::Owed(100)),
        ("untouched", shown(&[1])),
    ]);
    RecapCursor::default()
        .save(&settings, placing(start), Some(100))
        .unwrap();
    // two recaps read the same cursor; the second saves first
    let read = RecapCursor::read(&settings).unwrap();
    let second = marks(&[
        ("both-shown", shown(&[2])),
        ("owed-then-shown", shown(&[5])),
        ("shown-over-owed", Mark::Owed(100)),
        ("both-owed", Mark::Owed(150)),
    ]);
    read.save(&settings, placing(second), Some(200)).unwrap();
    let first = marks(&[
        ("both-shown", shown(&[3])),
        ("owed-then-shown", Mark::Owed(100)),
        ("shown-over-owed", shown(&[6])),
        ("only-first", shown(&[4])),
        ("both-owed", Mark::Owed(120)),
        ("new", shown(&[7])),
    ]);
    read.save(&settings, placing(first), Some(150)).unwrap();
    let saved = RecapCursor::read(&settings).unwrap();
    let expected = marks(&[
        // two reads of one store need not be nested: both frontiers stay
        ("both-shown", shown(&[2, 3])),
        // a recap that showed the tracker settled the window it was owed, whichever saved last
        ("owed-then-shown", shown(&[5])),
        ("shown-over-owed", shown(&[6])),
        // nobody else changed it since it was read: the later save replaces it
        ("only-first", shown(&[4])),
        ("both-owed", Mark::Owed(120)),
        ("new", shown(&[7])),
        ("untouched", shown(&[1])),
    ]);
    assert_eq!(saved.marks, expected);
    assert_eq!(saved.at, Some(200));
}

#[test]
fn a_frontier_that_is_not_the_one_read_replaces_it_when_nobody_raced() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings(home.path());
    RecapCursor::default()
        .save(
            &settings,
            placing(marks(&[("store", shown(&[5, 6]))])),
            Some(100),
        )
        .unwrap();
    // nobody else saved it since it was read: the frontier the recap made of it, which already
    // keeps what of the one it read it had to, replaces it as it is
    let read = RecapCursor::read(&settings).unwrap();
    read.save(
        &settings,
        placing(marks(&[("store", shown(&[2]))])),
        Some(200),
    )
    .unwrap();
    let saved = RecapCursor::read(&settings).unwrap();
    assert_eq!(saved.marks, marks(&[("store", shown(&[2]))]));
}

#[test]
fn a_save_of_owed_windows_alone_keeps_the_time_of_the_last_recap_that_marked_anything() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings(home.path());
    RecapCursor::default()
        .save(
            &settings,
            placing(marks(&[("shown", shown(&[1]))])),
            Some(100),
        )
        .unwrap();
    let read = RecapCursor::read(&settings).unwrap();
    read.save(&settings, placing(marks(&[("new", Mark::Owed(100))])), None)
        .unwrap();
    let saved = RecapCursor::read(&settings).unwrap();
    assert_eq!(saved.at, Some(100));
    let expected = marks(&[("new", Mark::Owed(100)), ("shown", shown(&[1]))]);
    assert_eq!(saved.marks, expected);
}

#[test]
fn a_save_of_no_marks_moves_only_the_time() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings(home.path());
    let kept = marks(&[("owed", Mark::Owed(50)), ("shown", shown(&[1]))]);
    RecapCursor::default()
        .save(&settings, placing(kept.clone()), Some(100))
        .unwrap();
    // a recap of everything that showed only what was seen before changed no mark
    let read = RecapCursor::read(&settings).unwrap();
    read.save(&settings, BTreeMap::new(), Some(200)).unwrap();
    let saved = RecapCursor::read(&settings).unwrap();
    assert_eq!((saved.marks, saved.at), (kept, Some(200)));
}

#[test]
fn a_save_keeps_of_a_frontier_saved_meanwhile_only_what_its_read_did_not_place() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings(home.path());
    // two recaps read the cursor before either saved; the one that read less saves first
    let read = RecapCursor::read(&settings).unwrap();
    let first = marks(&[("behind", shown(&[3])), ("beside", shown(&[2, 6]))]);
    read.save(&settings, placing(first), Some(100)).unwrap();
    // the second read 1 to 5 of both stores: everything the first did of one, and not 6 of the
    // other, which a sync wrote to a file it had read already
    let placed: Vec<EventId> = (1..=5).map(id).collect();
    let second = BTreeMap::from([
        ("behind".to_owned(), (shown(&[5]), placed.clone())),
        ("beside".to_owned(), (shown(&[5]), placed)),
    ]);
    read.save(&settings, second, Some(200)).unwrap();
    let saved = RecapCursor::read(&settings).unwrap();
    let expected = marks(&[("behind", shown(&[5])), ("beside", shown(&[5, 6]))]);
    assert_eq!(saved.marks, expected);
    assert_eq!(saved.at, Some(200));
}
