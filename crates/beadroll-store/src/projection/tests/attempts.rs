//! The accepted attempt files a reader takes (roll-lqjj.30): listed once the segments are read,
//! and each stat-ed once a locked phase.

use std::time::{Duration, Instant};

use super::*;

/// A reader outside the lock lists the attempt files once it has read the segments, whatever it
/// listed before: a claims directory that gains a file within one tick of a listing keeps its
/// identity, and a listing taken before the segments were read leaves out an attempt that a record
/// in them builds on (X13). Both ways a reader reads: a projection the record is applied after,
/// and the journal replayed.
#[test]
fn attempts_are_listed_afresh_once_the_segments_are_read() {
    for stored in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let ws = store(home.path(), Some("this build"));
        begun(&ws);
        if stored {
            assert!(replays(&ws));
        }
        let claims = ws.events_dir.join(HERE).join("claims");
        std::fs::create_dir_all(&claims).unwrap();
        // the reader's listing, before anything below landed
        let before = crate::claim_files(&ws).unwrap().to_vec();
        assert!(before.is_empty());
        // another process: an accepted attempt, then a record built on it
        let mut data = Object::new();
        data.insert("mode".into(), Value::from("online"));
        data.insert("generation".into(), Value::from(1));
        let claim = event(5, &[4], "claim", 2, data);
        write_attempt_file(&ws, &claims.join(format!("{}.json", id(5))), &[claim]).unwrap();
        append_events(&ws, HERE, &[title(6, &[5], 2, "after the claim")]).unwrap();
        // the directory kept its identity: the reader's listing still stands for it
        ws.claims.borrow_mut().seed_same_tick(&claims, before);
        let read = projection(&ws).unwrap().into_state();
        assert!(
            read.pending().is_empty(),
            "stored {stored}: {:?}",
            read.pending()
        );
        assert!(
            answers(&read).contains("after the claim"),
            "stored {stored}"
        );
        assert!(replays(&ws));
    }
}

/// The attempt files are listed after the segments are read, not merely afresh: a reader held
/// before it reads a segment (`journal.before-read`), while another process accepts an attempt and
/// appends a record built on it, reads that record, and so must find the attempt (X13). Both ways
/// a reader reads: the segment's tail applied to a projection, where an attempt it could not find
/// would have the journal replayed instead, and the journal replayed.
#[test]
fn a_record_read_from_the_segments_finds_the_attempt_it_builds_on() {
    for stored in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let ws = store(home.path(), Some("this build"));
        begun(&ws);
        if stored {
            assert!(replays(&ws));
        }
        // grown since the projection: the reader reads the segment's tail
        append_events(&ws, HERE, &[title(5, &[4], 2, "grown")]).unwrap();
        let claims = ws.events_dir.join(HERE).join("claims");
        std::fs::create_dir_all(&claims).unwrap();
        let gate = home.path().join("gate");
        let with = |suffix: &str| home.path().join(format!("gate{suffix}"));
        let spec = format!("journal.before-read=pause-until({})", gate.display());
        let read = std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let ws = failing(home.path(), Some("this build"), Some(&spec));
                let read = projection(&ws).unwrap().into_state();
                let reads = ws.settings.failpoints.hits("journal.before-read");
                (read.pending().is_empty(), answers(&read), reads)
            });
            let started = Instant::now();
            while !with(".ready").exists() && !reader.is_finished() {
                assert!(started.elapsed() < Duration::from_secs(60), "never paused");
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                with(".ready").exists(),
                "stored {stored}: never read a segment"
            );
            // another process: an accepted attempt, then a record built on it
            let mut data = Object::new();
            data.insert("mode".into(), Value::from("online"));
            data.insert("generation".into(), Value::from(1));
            let claim = event(6, &[5], "claim", 2, data);
            write_attempt_file(&ws, &claims.join(format!("{}.json", id(6))), &[claim]).unwrap();
            append_events(&ws, HERE, &[title(7, &[6], 2, "after the claim")]).unwrap();
            std::fs::write(with(".go"), b"").unwrap();
            reader.join().unwrap()
        });
        let (settled, answers, reads) = read;
        assert!(settled, "stored {stored}: {answers}");
        assert!(answers.contains("after the claim"), "stored {stored}");
        // the one segment read once: its tail applied to the projection, not the journal replayed
        assert_eq!(reads, 1, "stored {stored}: segments read");
        assert!(replays(&ws));
    }
}

/// Moves the projection's own time a minute past every file it was made from, so that each
/// identity it keeps is settled before it, whatever tick the files were written in.
fn settle(ws: &Workspace) {
    let later = std::time::SystemTime::now() + Duration::from_secs(60);
    let projection = std::fs::File::options()
        .write(true)
        .open(projection_path(ws))
        .unwrap();
    projection
        .set_times(std::fs::FileTimes::new().set_modified(later))
        .unwrap();
}

/// Under the store lock each attempt file is stat-ed once a phase, whoever asks, and again once
/// bead writes it: an attempt replaced under the lock, at the same length, is read as it is now
/// (roll-lqjj.30). Outside the lock nothing is kept, and a new phase stats every file again.
#[test]
fn an_attempt_is_stat_ed_once_a_locked_phase_and_again_once_bead_writes_it() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    let path = ws
        .events_dir
        .join(HERE)
        .join("claims")
        .join(format!("{}.json", id(5)));
    write_attempt_file(&ws, &path, &[title(5, &[4], 2, "before")]).unwrap();
    assert!(replays(&ws));
    assert_eq!(ws.claims.borrow().identities(), 0, "kept outside the lock");
    ws.set_holding(true);
    assert!(replays(&ws));
    assert_eq!(ws.claims.borrow().identities(), 1);
    settle(&ws);
    // replaced, as a merge replaces a damaged copy: another file, its length kept
    write_attempt_file(&ws, &path, &[title(5, &[4], 2, "AFTER!")]).unwrap();
    assert_eq!(ws.claims.borrow().identities(), 0, "forgotten by the write");
    let state = projection(&ws).unwrap().into_state();
    assert!(answers(&state).contains("AFTER!"));
    assert!(replays(&ws));
    ws.set_holding(false);
    assert_eq!(
        ws.claims.borrow().identities(),
        0,
        "forgotten with the lock"
    );
}
