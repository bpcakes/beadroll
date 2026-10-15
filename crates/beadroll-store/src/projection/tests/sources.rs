//! The journal files a projection was made from, kept beside it (roll-lqjj.37): whichever way it
//! was read, it is the store's current projection until bead writes a journal file, and not after.

use super::*;

use crate::fs::write_atomic;
use crate::listing::Keys;
use crate::projection::{Sourced, Sources, projection_sourced};

/// How the projection a test starts from was read.
#[derive(Clone, Copy, Debug)]
enum How {
    /// No projection was stored: the journal replayed.
    Replayed,
    /// The stored projection, whose files are the ones now.
    Stored,
    /// The stored projection, the records appended since applied after it.
    Appended,
    /// No projection is kept at all (no build): the journal replayed.
    Uncached,
}

/// A store with a segment and an attempt file, and a projection of it read as `how`.
fn read_as(home: &Path, how: How, locked: bool) -> (Workspace, Sourced) {
    let build = (!matches!(how, How::Uncached)).then_some("this build");
    let ws = store(home, build);
    begun(&ws);
    let mut claim = Object::new();
    claim.insert("mode".into(), Value::from("offline"));
    let attempt = [event(5, &[4], "claim", 2, claim)];
    write_attempt_file(&ws, &claimed(&ws, 5), &attempt).unwrap();
    if matches!(how, How::Stored | How::Appended) {
        assert!(replays(&ws));
    }
    if matches!(how, How::Appended) {
        append_events(&ws, HERE, &[title(6, &[5], 2, "later")]).unwrap();
    }
    ws.set_holding(locked);
    let read = projection_sourced(&ws).unwrap();
    (ws, read)
}

/// Where the attempt file of event `n` is.
fn claimed(ws: &Workspace, n: u128) -> PathBuf {
    let name = format!("{}.json", id(n));
    ws.events_dir.join(HERE).join("claims").join(name)
}

/// Replaces the file at `path` by one of the same bytes, as `write_atomic` does: a new file renamed
/// into place.
fn replaced(ws: &Workspace, path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    write_atomic(path, &bytes, true, &ws.settings.failpoints).unwrap();
}

type Write = fn(&Workspace);

/// What bead can write under `events/`, each a way the journal's files stop being the ones a
/// projection was made from.
fn writes() -> Vec<(&'static str, Write)> {
    vec![
        ("an event appended to a segment", |ws| {
            append_events(ws, HERE, &[title(7, &[6], 2, "window")]).unwrap();
        }),
        ("a segment of another machine", |ws| {
            let folder = ws.events_dir.join("0000bb02");
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("000001.jsonl"), b"").unwrap();
        }),
        ("a segment replaced by the same bytes", |ws| {
            replaced(ws, &segment(ws));
        }),
        ("a segment removed", |ws| {
            std::fs::remove_file(segment(ws)).unwrap();
        }),
        ("an attempt file written", |ws| {
            let mut claim = Object::new();
            claim.insert("mode".into(), Value::from("offline"));
            let attempt = [event(8, &[5], "claim", 3, claim)];
            write_attempt_file(ws, &claimed(ws, 8), &attempt).unwrap();
        }),
        ("an attempt file replaced by the same bytes", |ws| {
            let path = claimed(ws, 5);
            replaced(ws, &path);
            ws.forget_claims(&path);
        }),
        ("an attempt file removed", |ws| {
            let path = claimed(ws, 5);
            std::fs::remove_file(&path).unwrap();
            ws.forget_claims(&path);
        }),
    ]
}

/// A projection is the store's current one while the journal's files are the ones it was made
/// from, however it was read, in a locked phase (which keeps the attempt files' listing and
/// identities) and outside it.
#[test]
fn a_projection_is_current_while_the_journal_is_the_one_it_was_made_from() {
    for how in [How::Replayed, How::Stored, How::Appended, How::Uncached] {
        for locked in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let (ws, read) = read_as(home.path(), how, locked);
            let state = answers(read.state());
            let projection = read.into_current(&ws);
            let message = format!("{how:?}, locked {locked}");
            assert_eq!(
                projection.map(|p| answers(p.state())),
                Some(state),
                "{message}"
            );
        }
    }
}

/// Every write under `events/` makes the projection no longer the store's current one, for each of
/// the ways it was read: a caller that goes on from it reads the store again.
#[test]
fn a_projection_is_not_current_once_a_journal_file_is_written() {
    for how in [How::Replayed, How::Stored, How::Appended, How::Uncached] {
        for locked in [false, true] {
            for (what, write) in writes() {
                let home = tempfile::tempdir().unwrap();
                let (ws, read) = read_as(home.path(), how, locked);
                write(&ws);
                let message = format!("{how:?}, locked {locked}: {what}");
                assert!(read.into_current(&ws).is_none(), "{message}");
                // what the store gives instead is the journal as it is
                assert!(replays(&ws), "{message}");
            }
        }
    }
}

/// A projection read again after a write is the store's current one.
#[test]
fn a_projection_read_after_the_write_is_current() {
    let home = tempfile::tempdir().unwrap();
    let (ws, read) = read_as(home.path(), How::Stored, true);
    append_events(&ws, HERE, &[title(7, &[5], 2, "window")]).unwrap();
    assert!(read.into_current(&ws).is_none());
    let again = projection_sourced(&ws).unwrap();
    let state = answers(again.state());
    let projection = again.into_current(&ws).expect("current once read again");
    assert_eq!(answers(projection.state()), state);
    assert!(state.contains("window"), "{state}");
}

/// A projection with no identity for one of its files, or none at all, vouches for nothing: the
/// store is read again.
#[test]
fn a_projection_that_cannot_name_its_files_is_not_current() {
    let home = tempfile::tempdir().unwrap();
    let (ws, read) = read_as(home.path(), How::Stored, true);
    let projection = read.into_projection();
    assert!(Sourced::new(projection, None).into_current(&ws).is_none());
    let nameless: Keys = Arc::from([None]);
    assert!(Sources::of(Vec::new(), Arc::from([PathBuf::from("a")]), nameless).is_none());
}
