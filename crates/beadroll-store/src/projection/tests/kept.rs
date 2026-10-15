//! When a projection caught up with the records after it is written back (roll-lqjj.36, X17): by a
//! command that reads, whatever it caught up; by one that extends it to write, only once those
//! records hold more than a [`TAIL`]th of its bytes.

use super::super::{TAIL, Want, kept_for};
use super::*;

#[test]
fn a_read_writes_back_what_it_caught_up_and_a_write_only_many_records() {
    let size = 64 * 1000;
    for want in [Want::Texts, Want::Outline] {
        for tail in [0, 1, 1000, 1001, u64::MAX] {
            assert!(kept_for(want, tail, size), "{tail}");
        }
    }
    for (tail, kept) in [
        (0, false),
        (1, false),
        (1000, false),
        (1001, true),
        (u64::MAX, true),
    ] {
        assert_eq!(kept_for(Want::Projection, tail, size), kept, "{tail}");
    }
}

/// The bytes of the store's one segment.
fn length(ws: &Workspace) -> u64 {
    std::fs::metadata(segment(ws)).unwrap().len()
}

/// Writes one after another, each finding the records the ones before it appended: the stored
/// projection stays as it was until those records hold more than a 64th of its bytes, every write
/// answering as replay all the same; then a write writes it back, and a read after any write does.
#[test]
fn a_projection_caught_up_for_a_write_is_written_back_once_the_records_after_it_are_many() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    // a tracker whose projection a record is a small part of
    let creates: Vec<Value> = (5..205)
        .map(|n| create(n, &[n - 1], &format!("pr-{n}")))
        .collect();
    append_events(&ws, HERE, &creates).unwrap();
    assert!(replays(&ws), "a read writes it back");
    let replayed = |ws: &Workspace| answers(&replay(load_journal(ws).unwrap()).unwrap());
    let mut stored = std::fs::read(projection_path(&ws)).unwrap();
    let mut since = length(&ws);
    let (mut left, mut kept) = (0, 0);
    for n in 205..245 {
        append_events(&ws, HERE, &[title(n, &[n - 1], 5, &format!("title {n}"))]).unwrap();
        let mut caught = projection(&ws).unwrap().into_state();
        let uids: Vec<EventId> = caught.issues().map(Issue::uid).collect();
        assert!(read_texts(&ws, &mut caught, uids));
        assert_eq!(answers(&caught), replayed(&ws), "write {n}");
        let now = std::fs::read(projection_path(&ws)).unwrap();
        if (length(&ws) - since) * TAIL > stored.len() as u64 {
            assert_ne!(now, stored, "write {n}: the records after it are many");
            (stored, since, kept) = (now, length(&ws), kept + 1);
        } else {
            assert_eq!(now, stored, "write {n}: the records after it are few");
            left += 1;
        }
    }
    assert!(left >= 20 && kept >= 2, "{left} left, {kept} written back");
    if length(&ws) == since {
        append_events(&ws, HERE, &[title(245, &[244], 5, "after")]).unwrap();
    }
    assert_eq!(answers(&view(&ws).unwrap()), replayed(&ws));
    assert_ne!(std::fs::read(projection_path(&ws)).unwrap(), stored);
    assert_eq!(made(&ws).unwrap().values().sum::<u64>(), length(&ws));
}
