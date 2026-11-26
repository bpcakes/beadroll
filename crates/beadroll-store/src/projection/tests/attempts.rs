//! The accepted attempt files a reader takes (roll-lqjj.30): listed once the segments are read,
//! and each stat-ed once a locked phase.

use std::time::{Duration, Instant};

use super::*;
use crate::LockState;

/// Replay must fail if it cannot buffer an accepted attempt. A retry sees the confirmed claim
/// in that same file (AU01).
#[test]
fn an_allocation_failure_reading_an_accepted_attempt_fails_the_view() {
    let home = tempfile::tempdir().unwrap();
    let ws = failing(
        home.path(),
        Some("this build"),
        Some("journal.read-attempt=errno(ENOMEM)"),
    );
    begun(&ws);
    let mut data = Object::new();
    data.insert("mode".into(), Value::from("online"));
    let claim = [event(5, &[4], "claim", 2, data)];
    let path = crate::attempt_path(&ws, &claim);
    write_attempt_file(&ws, &path, &claim).unwrap();

    let errno = rustix::io::Errno::NOMEM.raw_os_error();
    let expected = beadroll_git::IoError::at(&std::io::Error::from_raw_os_error(errno), &path);
    assert_eq!(
        view(&ws).unwrap_err(),
        crate::ViewError::Store(crate::Error::Io(expected))
    );
    assert_eq!(ws.settings.failpoints.hits("journal.read-attempt"), 1);
    let state = view(&ws).unwrap();
    assert_eq!(
        state
            .issue_named(&id(2))
            .unwrap()
            .claim()
            .unwrap()
            .id
            .to_string(),
        id(5)
    );
}

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
    crate::testing::set_modified(&projection_path(ws), later);
}

/// A stored projection waiting for a missing cause can answer an unchanged read. When a
/// segment grows or an attempt arrives, only full replay reads the new records (AU11).
#[test]
fn a_waiting_projection_reads_changed_records_only_during_replay() {
    for kind in [JournalFile::Segment, JournalFile::Attempt] {
        let home = tempfile::tempdir().unwrap();
        let ws = store(home.path(), Some("this build"));
        begun(&ws);
        append_events(&ws, HERE, &[title(6, &[4, 99], 2, "waiting")]).unwrap();
        assert!(!projection(&ws).unwrap().extendable());
        settle(&ws);

        let reader = failing(
            home.path(),
            Some("this build"),
            Some("journal.before-read@always=sleep(0);journal.read-attempt@always=sleep(0)"),
        );
        let reads = |point| reader.settings.failpoints.hits(point);
        assert!(!projection(&reader).unwrap().extendable());
        assert_eq!(reads("journal.before-read"), 0);
        assert_eq!(reads("journal.read-attempt"), 0);

        let next = [title(7, &[6], 3, "also waiting")];
        match kind {
            JournalFile::Segment => append_events(&ws, HERE, &next).unwrap(),
            JournalFile::Attempt => {
                write_attempt_file(&ws, &crate::attempt_path(&ws, &next), &next).unwrap();
            }
        }
        let projected = projection(&reader).unwrap();
        let replayed = replay(load_journal(&ws).unwrap()).unwrap();
        assert!(
            replayed
                .pending()
                .contains(&EventId::parse(&id(7)).unwrap())
        );
        assert_eq!(projected.state().pending(), replayed.pending());
        assert_eq!(reads("journal.before-read"), 1, "{kind:?}");
        assert_eq!(
            reads("journal.read-attempt"),
            u64::from(kind == JournalFile::Attempt)
        );
    }
}

/// A file of the projection's own timestamp can still match its checksum. A waiting
/// projection verifies that file and reuses the other settled segments (AU14).
#[test]
fn a_waiting_projection_only_verifies_the_file_of_its_own_tick() {
    for kind in [JournalFile::Segment, JournalFile::Attempt] {
        let home = tempfile::tempdir().unwrap();
        let ws = store(home.path(), Some("this build"));
        begun(&ws);
        let waiting = [title(6, &[4, 99], 2, "waiting")];
        let path = match kind {
            JournalFile::Segment => {
                let path = ws.events_dir.join(HERE).join("000002.jsonl");
                std::fs::write(&path, crate::journal::encode_events(&waiting).unwrap()).unwrap();
                path
            }
            JournalFile::Attempt => {
                let path = crate::attempt_path(&ws, &waiting);
                write_attempt_file(&ws, &path, &waiting).unwrap();
                path
            }
        };
        let tick = std::time::SystemTime::now() + Duration::from_secs(60);
        crate::testing::set_modified(&path, tick);
        assert!(!projection(&ws).unwrap().extendable());
        crate::testing::set_modified(&projection_path(&ws), tick);

        let reader = failing(
            home.path(),
            Some("this build"),
            Some("journal.before-read@always=sleep(0);journal.read-attempt@always=sleep(0)"),
        );
        let state = view(&reader).unwrap();
        assert!(state.pending().contains(&EventId::parse(&id(6)).unwrap()));
        assert_eq!(
            reader.settings.failpoints.hits("journal.before-read"),
            u64::from(kind == JournalFile::Segment)
        );
        assert_eq!(reader.settings.failpoints.hits("journal.read-attempt"), 0);
    }
}

/// A removed or replaced attempt invalidates the saved sources before the index is read (AU15).
#[test]
fn a_changed_attempt_replays_without_reading_the_saved_index() {
    for replacement in [None, Some("replaced")] {
        let home = tempfile::tempdir().unwrap();
        let ws = store(home.path(), Some("this build"));
        begun(&ws);
        let event = [title(5, &[4], 2, "initial")];
        let path = crate::attempt_path(&ws, &event);
        write_attempt_file(&ws, &path, &event).unwrap();
        assert!(projection(&ws).unwrap().extendable());
        settle(&ws);
        let reader = failing(
            home.path(),
            Some("this build"),
            Some("projection.read-index@always=sleep(0);journal.before-read@always=sleep(0)"),
        );
        assert!(answers(projection(&reader).unwrap().state()).contains("initial"));
        let reads = |point| reader.settings.failpoints.hits(point);
        assert_eq!(reads("projection.read-index"), 1);
        assert_eq!(reads("journal.before-read"), 0);

        match replacement {
            Some(text) => write_attempt_file(&ws, &path, &[title(5, &[4], 2, text)]).unwrap(),
            None => std::fs::remove_file(&path).unwrap(),
        }
        let answer = answers(projection(&reader).unwrap().state());
        assert!(!answer.contains("initial"));
        if let Some(text) = replacement {
            assert!(answer.contains(text));
        }
        assert_eq!(reads("projection.read-index"), 1);
        assert_eq!(reads("journal.before-read"), 1);
    }
}

/// Even when the file identity still matches, a waiting cache must verify its attempt's saved
/// checksum before reuse (AU20).
#[test]
fn a_waiting_projection_replays_on_an_attempt_checksum_mismatch() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    begun(&ws);
    let waiting = [title(6, &[4, 99], 2, "waiting")];
    let path = crate::attempt_path(&ws, &waiting);
    write_attempt_file(&ws, &path, &waiting).unwrap();
    let tick = std::time::SystemTime::now() + Duration::from_secs(60);
    crate::testing::set_modified(&path, tick);
    let mut cached = projection(&ws).unwrap();
    assert!(!cached.extendable());
    let mut made = file::read(&projection_path(&ws)).unwrap().made;
    let attempt = made
        .files
        .iter_mut()
        .find(|f| f.kind == JournalFile::Attempt)
        .unwrap();
    assert_eq!(FileKey::of(&path), Some(attempt.key));
    attempt.sum.as_mut().unwrap()[0] ^= 1;
    super::super::save(&ws, made, &mut cached);
    crate::testing::set_modified(&projection_path(&ws), tick);

    let reader = failing(
        home.path(),
        Some("this build"),
        Some("projection.read-index@always=sleep(0);journal.read-attempt@always=sleep(0)"),
    );
    assert!(
        view(&reader)
            .unwrap()
            .pending()
            .contains(&EventId::parse(&id(6)).unwrap())
    );
    assert_eq!(reader.settings.failpoints.hits("journal.read-attempt"), 1);
    assert_eq!(reader.settings.failpoints.hits("projection.read-index"), 0);
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
    ws.set_holding(LockState::Locked);
    assert!(replays(&ws));
    assert_eq!(ws.claims.borrow().identities(), 1);
    settle(&ws);
    // replaced, as a merge replaces a damaged copy: another file, its length kept
    write_attempt_file(&ws, &path, &[title(5, &[4], 2, "AFTER!")]).unwrap();
    assert_eq!(ws.claims.borrow().identities(), 0, "forgotten by the write");
    let state = projection(&ws).unwrap().into_state();
    assert!(answers(&state).contains("AFTER!"));
    assert!(replays(&ws));
    ws.set_holding(LockState::Unlocked);
    assert_eq!(
        ws.claims.borrow().identities(),
        0,
        "forgotten with the lock"
    );
}
