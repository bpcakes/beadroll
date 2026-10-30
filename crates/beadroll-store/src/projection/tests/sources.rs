//! The journal files a projection was made from, kept beside it (roll-lqjj.37): whichever way it
//! was read, it is the store's current projection until bead writes a journal file, and not after.

use super::*;
use crate::FileMode;
use crate::LockState;
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
fn read_as(home: &Path, how: How, lock_state: LockState) -> (Workspace, Sourced) {
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
    ws.set_holding(lock_state);
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
    write_atomic(path, &bytes, FileMode::Private, &ws.settings.failpoints).unwrap();
}

/// Puts the modification time of the directory `dir` back a minute, so that a change made in it
/// is seen however coarse the clock its times come from is: a directory that gains a file within
/// one tick of its listing keeps its identity (X13).
fn aged(dir: &Path) {
    let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    let opened = std::fs::File::open(dir).unwrap();
    let times = std::fs::FileTimes::new().set_modified(earlier);
    opened.set_times(times).unwrap();
}

/// The claims directory of this machine.
fn claims(ws: &Workspace) -> PathBuf {
    ws.events_dir.join(HERE).join("claims")
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
        // in place, by a writer that tells the phase (`Workspace::forget_claims`): the file is
        // stat-ed again, and only the identity kept of it says it is not the one read
        ("an attempt file rewritten in place, announced", |ws| {
            let path = claimed(ws, 5);
            let text = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, text.replace("offline", "online")).unwrap();
            ws.forget_claims(&path);
        }),
        // by a writer that does not tell the phase (`Workspace::forget_claims`)
        (
            "an attempt file replaced by the same bytes, unannounced",
            |ws| {
                replaced(ws, &claimed(ws, 5));
                aged(&claims(ws));
            },
        ),
        ("an attempt file removed, unannounced", |ws| {
            std::fs::remove_file(claimed(ws, 5)).unwrap();
            aged(&claims(ws));
        }),
        (
            "an attempt file added to a claims directory, unannounced",
            |ws| {
                std::fs::write(claimed(ws, 8), b"{}\n").unwrap();
                aged(&claims(ws));
            },
        ),
        ("a claims directory of another machine, unannounced", |ws| {
            let folder = ws.events_dir.join("0000bb02").join("claims");
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join(format!("{}.json", id(8))), b"{}\n").unwrap();
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
            let (ws, read) = read_as(
                home.path(),
                how,
                if locked {
                    LockState::Locked
                } else {
                    LockState::Unlocked
                },
            );
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
                let (ws, read) = read_as(
                    home.path(),
                    how,
                    if locked {
                        LockState::Locked
                    } else {
                        LockState::Unlocked
                    },
                );
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
    let (ws, read) = read_as(home.path(), How::Stored, LockState::Locked);
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
    let (ws, read) = read_as(home.path(), How::Stored, LockState::Locked);
    let projection = read.into_projection();
    assert!(Sourced::new(projection, None).into_current(&ws).is_none());
    let nameless: Keys = Arc::from([None]);
    let (paths, stamp) = (Arc::from([PathBuf::from("a")]), Some(Arc::from([])));
    assert!(Sources::of(Vec::new(), paths, nameless, stamp).is_none());
    // nor does one whose claims directories could not be told
    let keys: Keys = Arc::from([]);
    assert!(Sources::of(Vec::new(), Arc::from([]), keys, None).is_none());
}

/// Each comparison of the sources is made on its own: the sources of a projection read from a store
/// that has not changed, altered in one respect only, are no longer the store's. A comparison the
/// writes above are all caught by another of is pinned here: the names of the attempt files, which
/// the identities and the directories also say a file was added or removed by, and the identities,
/// which alone see a file rewritten in place that the phase was told of.
#[test]
fn each_comparison_of_the_sources_is_made() {
    type Alter = fn(&mut Sources);
    let alterations: [(&str, Alter); 5] = [
        ("a segment's identity", |sources| {
            let (_, key) = &mut sources.segments[0];
            *key = key.up_to(key.len() + 1);
        }),
        ("a segment's name", |sources| {
            sources.segments[0].0.push('x');
        }),
        ("an attempt file's name", |sources| {
            let mut names = sources.attempts.to_vec();
            names[0].set_extension("other");
            sources.attempts = names.into();
        }),
        ("an attempt file's identity", |sources| {
            let mut keys = sources.keys.to_vec();
            keys[0] = keys[0].map(|key| key.up_to(key.len() + 1));
            sources.keys = keys.into();
        }),
        ("a claims directory's identity", |sources| {
            sources.stamp = Arc::from([]);
        }),
    ];
    for how in [How::Replayed, How::Stored, How::Appended, How::Uncached] {
        for locked in [false, true] {
            for (what, alter) in alterations {
                let home = tempfile::tempdir().unwrap();
                let (ws, mut read) = read_as(
                    home.path(),
                    how,
                    if locked {
                        LockState::Locked
                    } else {
                        LockState::Unlocked
                    },
                );
                alter(read.sources.as_mut().expect("sources"));
                let message = format!("{how:?}, locked {locked}: {what}");
                assert!(read.into_current(&ws).is_none(), "{message}");
            }
        }
    }
}
