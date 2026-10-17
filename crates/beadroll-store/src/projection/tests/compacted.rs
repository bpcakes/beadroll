//! What a projection written back keeps of what every event had seen (roll-lqjj.43, X17): every
//! event it knew, as it was when last compacted, and every event taken in since, until those are
//! more than an eighth of the events it knew then; the write-back that finds them so compacts it.
//! A read that caught it up and a write that writes it back follow the same rule, being the same
//! write-back.

use beadroll_core::projection::{COMPACTED, Projection};

use super::*;

/// The projection the store holds, read back.
fn stored(ws: &Workspace) -> Projection {
    let stored = file::read(&projection_path(ws)).unwrap();
    Projection::decode(stored.encoded().unwrap()).unwrap()
}

#[test]
fn a_write_back_compacts_the_knowledge_only_once_it_holds_more_than_an_eighth_more() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    let creates: Vec<Value> = (5..45)
        .map(|n| create(n, &[n - 1], &format!("pr-{n}")))
        .collect();
    append_events(&ws, HERE, &creates).unwrap();
    assert!(replays(&ws), "made whole, and written back");
    let mut last = stored(&ws);
    assert_eq!(last.known(), last.compacted());
    let replayed = |ws: &Workspace| answers(&replay(load_journal(ws).unwrap()).unwrap());
    let (mut taken, mut compactions, mut left, mut by_writes) = (0, 0, 0, 0);
    for n in 45..240 {
        // each retitle leaves the title before it named no more
        append_events(&ws, HERE, &[title(n, &[n - 1], 5, &format!("title {n}"))]).unwrap();
        taken += 1;
        // runs of writes, which write back once the records after the stored projection are many,
        // and of reads, which write back each time
        let writing = (n / 16) % 2 == 0;
        let state = if writing {
            let mut state = projection(&ws).unwrap().into_state();
            let uids: Vec<EventId> = state.issues().map(Issue::uid).collect();
            assert!(read_texts(&ws, &mut state, uids));
            state
        } else {
            view(&ws).unwrap()
        };
        assert_eq!(answers(&state), replayed(&ws), "event {n}");
        let now = stored(&ws);
        if now.event_count() == last.event_count() {
            assert!(writing, "event {n}: a read writes back what it caught up");
            continue;
        }
        let since = last.known() - last.compacted() + taken;
        if since * COMPACTED > last.compacted() {
            assert_eq!(now.known(), now.compacted(), "event {n}");
            assert!(now.known() < last.known() + taken, "event {n}");
            compactions += 1;
        } else {
            let kept = (now.known(), now.compacted());
            assert_eq!(kept, (last.known() + taken, last.compacted()), "event {n}");
            left += 1;
        }
        by_writes += usize::from(writing);
        (last, taken) = (now, 0);
    }
    assert!(
        compactions >= 10 && left >= 50,
        "{compactions} compactions, {left} left"
    );
    assert!(by_writes >= 3, "{by_writes} written back by writes");
}
