//! Each issue's texts, kept apart from the projection's state (roll-lqjj.6): read in only where a
//! command needs them, a write adding its own to the text file, and a text file damaged, cut, gone
//! or another's never trusted, while what a list shows needs none of it.

use super::*;

use crate::projection::{SLACK, outline, state, texts_path};

fn described(n: u128, deps: &[u128], issue: u128, text: &str) -> Value {
    let mut fields = Object::new();
    fields.insert("description".into(), Value::from(text));
    let mut data = Object::new();
    data.insert("fields".into(), Value::from(fields));
    event(n, deps, "set", issue, data)
}

fn comment(n: u128, deps: &[u128], issue: u128, text: &str) -> Value {
    let mut data = Object::new();
    data.insert("text".into(), Value::from(text));
    event(n, deps, "comment", issue, data)
}

/// [`begun`], its first issue described and commented on: the frontier is event 6.
fn written(ws: &Workspace) {
    begun(ws);
    let events = [
        described(5, &[4], 2, "why it matters, at length"),
        comment(6, &[5], 2, "noted"),
    ];
    append_events(ws, HERE, &events).unwrap();
}

/// What a list shows of each issue of `state`, by key.
fn listed(state: &State) -> Vec<String> {
    (state.issues_by_key().iter())
        .map(|issue| format!("{:?}", state.brief(issue)))
        .collect()
}

fn uid(n: u128) -> EventId {
    EventId::parse(&id(n)).unwrap()
}

#[test]
fn texts_are_read_in_only_where_a_command_needs_them() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    written(&ws);
    assert!(replays(&ws));
    assert!(texts_path(&ws).is_file());
    let outlined = outline(&ws).unwrap();
    assert!(outlined.issues().all(|issue| !issue.texts().here()));
    let whole = state(&ws).unwrap();
    assert!(whole.issues().all(|issue| issue.texts().here()));
    assert_eq!(listed(&outlined), listed(&whole));
    // one issue's read in, and the other's still there
    let mut one = outline(&ws).unwrap();
    assert!(read_texts(&ws, &mut one, [uid(2)]));
    let first = one.issue(uid(2)).unwrap();
    assert!(first.texts().here());
    assert_eq!(first.comments().len(), 1);
    assert_eq!(first, whole.issue(uid(2)).unwrap());
    assert!(!one.issue(uid(3)).unwrap().texts().here());
}

#[test]
fn a_write_adds_its_texts_to_the_file() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    written(&ws);
    assert!(replays(&ws));
    let before = std::fs::read(texts_path(&ws)).unwrap();
    append_events(&ws, HERE, &[comment(7, &[6], 3, "another")]).unwrap();
    assert!(replays(&ws));
    let after = std::fs::read(texts_path(&ws)).unwrap();
    assert!(after.len() > before.len());
    assert_eq!(
        after[..before.len()],
        before[..],
        "appended to, nothing written again"
    );
}

#[test]
fn a_text_file_damaged_cut_gone_or_anothers_is_never_trusted() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    written(&ws);
    assert!(replays(&ws));
    let replayed = listed(&replay(load_journal(&ws).unwrap()).unwrap());
    let path = texts_path(&ws);
    let good = std::fs::read(&path).unwrap();
    // every cut, and a byte changed anywhere: its generation changed is another's file
    let mut damaged: Vec<Vec<u8>> = (0..good.len()).map(|cut| good[..cut].to_vec()).collect();
    for at in 0..good.len() {
        let mut flipped = good.clone();
        flipped[at] ^= 0x01;
        damaged.push(flipped);
    }
    for bytes in damaged {
        std::fs::write(&path, &bytes).unwrap();
        // what a list shows needs none of it
        assert_eq!(listed(&outline(&ws).unwrap()), replayed);
        // texts that cannot be read in are no texts: a write about them would replay
        let mut projected = projection(&ws).unwrap().into_state();
        assert!(!read_texts(&ws, &mut projected, [uid(2), uid(3)]));
        // a command that reads them all replays, and writes both files again
        assert_eq!(listed(&view(&ws).unwrap()), replayed);
        assert_eq!(std::fs::read(&path).unwrap(), good);
        assert!(replays(&ws));
    }
    std::fs::remove_file(&path).unwrap();
    assert_eq!(listed(&view(&ws).unwrap()), replayed);
    assert_eq!(std::fs::read(&path).unwrap(), good);
    assert!(replays(&ws));
}

#[test]
fn a_text_file_mostly_no_longer_read_is_started_again() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    written(&ws);
    assert!(replays(&ws));
    // the first issue described again and again, at length, each time caught up alone
    let long = "x".repeat(64 * 1024);
    let rewrites = 2 * SLACK / (64 * 1024) + 4;
    for n in 0..rewrites {
        let at = 7 + n as u128;
        append_events(&ws, HERE, &[described(at, &[at - 1], 2, &long)]).unwrap();
        projection(&ws).unwrap();
    }
    let len = std::fs::metadata(texts_path(&ws)).unwrap().len();
    assert!(
        len < 2 * SLACK,
        "{len} bytes after {rewrites} descriptions of 64 KiB"
    );
    assert!(replays(&ws));
}

/// Neither file is flushed (X05), so a crash can lose the texts appended before a projection that
/// places them and keep the projection: the text file is then shorter than what it places, and the
/// next texts appended go where the lost ones were. Read there, they are another issue's, and fail
/// the sum kept of the lost ones: never read as theirs, and the journal is replayed.
#[test]
fn texts_lost_behind_a_projection_are_never_read_as_its_own() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    written(&ws);
    assert!(replays(&ws));
    let before = std::fs::read(texts_path(&ws)).unwrap();
    append_events(&ws, HERE, &[comment(7, &[6], 3, "another")]).unwrap();
    assert!(replays(&ws));
    // the comment's texts lost, the projection placing them kept
    std::fs::write(texts_path(&ws), &before).unwrap();
    let mut short = outline(&ws).unwrap();
    assert!(
        !read_texts(&ws, &mut short, [uid(3)]),
        "placed past its end"
    );
    // the first issue described again: its texts are appended where the comment's were
    let again = "why it matters, said again at greater length";
    append_events(&ws, HERE, &[described(8, &[7], 2, again)]).unwrap();
    // a read takes it in, and writes back the projection with the texts it appended
    let mut extended = outline(&ws).unwrap();
    let wanted = extended.wanted_texts([uid(3)]);
    let [(_, place)] = wanted[..] else {
        panic!("{wanted:?}")
    };
    assert_eq!(place.at, before.len() as u64, "where the lost texts were");
    let len = std::fs::metadata(texts_path(&ws)).unwrap().len();
    assert!(
        len >= place.at + u64::from(place.len),
        "other texts there now"
    );
    assert!(!read_texts(&ws, &mut extended, [uid(3)]));
    let replayed = answers(&replay(load_journal(&ws).unwrap()).unwrap());
    assert_eq!(answers(&view(&ws).unwrap()), replayed);
    assert!(replays(&ws));
}

/// Neither file is flushed, so a crash can leave each of them lost, cut, zeroed, as it was before
/// the last write or after it, whatever became of the other: a projection newer than its text
/// file, older, or of another generation. Every pair is read as replay answers, or not at all and
/// the journal replayed, by each kind of command: one that lists, one that shows some issues, one
/// that shows them all, and one that writes. A projection of the journal as it is reads texts in
/// from its own text file alone.
#[test]
fn whatever_a_crash_leaves_of_the_two_files_answers_as_replay() {
    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    let both = || {
        let read = |path: PathBuf| std::fs::read(path).unwrap();
        (read(projection_path(&ws)), read(texts_path(&ws)))
    };
    written(&ws);
    assert!(replays(&ws));
    let (first, first_texts) = both();
    // a comment's texts appended to the text file, and the projection written after them
    append_events(&ws, HERE, &[comment(7, &[6], 3, "another")]).unwrap();
    assert!(replays(&ws));
    let (appended, appended_texts) = both();
    assert!(appended_texts.starts_with(&first_texts));
    // the projection made whole, and its texts written to a text file of another generation
    append_events(&ws, HERE, &[comment(8, &[7], 2, "and more")]).unwrap();
    std::fs::remove_file(projection_path(&ws)).unwrap();
    assert!(replays(&ws));
    let (whole, whole_texts) = both();
    assert_ne!(whole_texts[16..32], first_texts[16..32]);
    let cut = |bytes: &[u8]| Some(bytes[..bytes.len() / 2].to_vec());
    let zeroed = |bytes: &[u8]| Some(vec![0; bytes.len()]);
    let mut lost_tail = appended_texts.clone();
    lost_tail[first_texts.len()..].fill(0);
    let projections = [
        Some(first),
        Some(appended),
        Some(whole.clone()),
        cut(&whole),
        zeroed(&whole),
        Some(Vec::new()),
        None,
    ];
    let (current, its_texts) = (2, 3);
    let texts = [
        Some(first_texts),
        Some(appended_texts),
        Some(lost_tail),
        Some(whole_texts.clone()),
        cut(&whole_texts),
        zeroed(&whole_texts),
        Some(Vec::new()),
        None,
    ];
    let replayed = replay(load_journal(&ws).unwrap()).unwrap();
    let uids: Vec<EventId> = replayed.issues().map(Issue::uid).collect();
    let leave = |p: &Option<Vec<u8>>, t: &Option<Vec<u8>>| {
        for (path, bytes) in [(projection_path(&ws), p), (texts_path(&ws), t)] {
            match bytes {
                Some(bytes) => std::fs::write(path, bytes).unwrap(),
                None => drop(std::fs::remove_file(path)),
            }
        }
    };
    for (p, projected) in projections.iter().enumerate() {
        for (t, texts) in texts.iter().enumerate() {
            let case = format!("projection {p}, text file {t}");
            leave(projected, texts);
            assert_eq!(listed(&outline(&ws).unwrap()), listed(&replayed), "{case}");
            leave(projected, texts);
            let mut some = outline(&ws).unwrap();
            let read = read_texts(&ws, &mut some, uids.iter().copied());
            if p == current {
                assert_eq!(read, t == its_texts, "{case}");
            }
            if read {
                assert_eq!(answers(&some), answers(&replayed), "{case}");
            }
            leave(projected, texts);
            assert_eq!(answers(&view(&ws).unwrap()), answers(&replayed), "{case}");
            leave(projected, texts);
            let mut writing = projection(&ws).unwrap().into_state();
            if read_texts(&ws, &mut writing, uids.iter().copied()) {
                assert_eq!(answers(&writing), answers(&replayed), "{case}");
            }
            // the first command that needs texts it cannot read replays, and writes both again
            assert_eq!(answers(&view(&ws).unwrap()), answers(&replayed), "{case}");
            assert!(replays(&ws), "{case}");
        }
    }
}

/// Both files are written past the failpoints of an atomic write, before and after the rename:
/// the text file when it is started, the projection each time; texts appended pass
/// `projection.texts-appended` instead. A text file that cannot be started has no projection
/// written to place texts in it.
#[test]
fn both_files_pass_the_failpoints_of_an_atomic_write() {
    let home = tempfile::tempdir().unwrap();
    // the text file's points first: the projection's part names it too
    let points = [
        "atomic-write:/projection-texts",
        "atomic-written:/projection-texts",
        "atomic-write:/projection",
        "atomic-written:/projection",
        "projection.texts-appended",
    ];
    let specs: Vec<String> = (points.iter())
        .map(|point| format!("{point}@always=sleep(0)"))
        .collect();
    let ws = failing(home.path(), Some("this build"), Some(&specs.join(";")));
    let hits = || points.map(|point| ws.settings.failpoints.hits(point));
    written(&ws);
    assert!(replays(&ws));
    assert_eq!(
        hits(),
        [1, 1, 1, 1, 0],
        "started, and the projection written"
    );
    append_events(&ws, HERE, &[comment(7, &[6], 3, "another")]).unwrap();
    assert!(replays(&ws));
    assert_eq!(
        hits(),
        [1, 1, 2, 2, 1],
        "appended to, and the projection written"
    );
    // the volume full as the text file is started
    let home = tempfile::tempdir().unwrap();
    let full = failing(
        home.path(),
        Some("this build"),
        Some("atomic-write:/projection-texts@always=errno(ENOSPC)"),
    );
    written(&full);
    assert!(replays(&full));
    assert!(!texts_path(&full).exists());
    assert!(!projection_path(&full).exists());
    let left: Vec<_> = std::fs::read_dir(&full.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(
        !left.iter().any(|n| n.to_string_lossy().ends_with(".tmp")),
        "{left:?}"
    );
}

/// roll-lqjj.52: a write-back removes what a bead killed writing either cache left beside it,
/// both caches' at once, whether it appends to the text file or starts it again; what a writer
/// still running has is kept.
#[test]
fn a_write_back_removes_what_killed_writers_of_either_cache_left() {
    use std::os::unix::fs::MetadataExt as _;

    use crate::fs::abandoned::testing::{ended, process};

    let home = tempfile::tempdir().unwrap();
    let ws = store(home.path(), Some("this build"));
    written(&ws);
    assert!(replays(&ws));
    let running = process();
    let (live, dead) = (running.id(), ended(process()));
    let left = |pid: u32| {
        [
            format!(".projection.{pid}.tmp"),
            format!(".projection-texts.{pid}.tmp"),
        ]
    };
    for name in left(live).iter().chain(&left(dead)) {
        std::fs::write(ws.dir.join(name), "left behind").unwrap();
    }
    let temporaries = || {
        let mut names: Vec<String> = (std::fs::read_dir(&ws.dir).unwrap())
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        names.sort();
        names
    };
    // a comment appended: the next read takes it in, appends its text to the text file, and writes
    // the projection back; the dead writer's files go, the running one's stay
    append_events(&ws, HERE, &[comment(7, &[6], 2, "another")]).unwrap();
    let before = std::fs::metadata(texts_path(&ws)).unwrap();
    assert!(replays(&ws));
    let after = std::fs::metadata(texts_path(&ws)).unwrap();
    assert!(after.ino() == before.ino() && after.len() > before.len());
    let mut kept = left(live);
    kept.sort();
    assert_eq!(temporaries(), kept);
    // that writer ends, and the text file is started again: nothing stays
    ended(running);
    std::fs::remove_file(projection_path(&ws)).unwrap();
    assert!(replays(&ws));
    assert_ne!(
        std::fs::metadata(texts_path(&ws)).unwrap().ino(),
        after.ino()
    );
    assert_eq!(temporaries(), Vec::<String>::new());
}
