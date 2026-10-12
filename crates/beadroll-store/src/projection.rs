//! The store's projection (roll-lqjj.2, step 3 of epic roll-lqjj): the state its journal replays
//! into, kept in `<store>/projection` with what it was made from (the build of bead that made it,
//! and every journal file's identity on disk, as the memo of roll-lqjj.1 compares files), so that a
//! command reads it instead of replaying the whole journal. It is a cache, never a source.
//!
//! A command lists the journal's files as replay does and compares them with the ones the
//! projection names. All the same: the projection is read, a command that only reads reading its
//! state alone. Records appended since (a segment grown, a segment or an attempt file added) are
//! read alone and applied after it, when replay would apply them so
//! ([`Projection::extend`]). Anything else (a file replaced, rewritten, cut or gone, a record
//! replay would apply otherwise) and a projection that is missing, unreadable, damaged or written
//! by another build of bead: the journal is replayed. A projection made is written back, whole or
//! not at all, when the files it was made from held still while they were read and every segment
//! ended with a whole line, never into a store that is not there. Deleting it is safe at any
//! moment.

mod file;

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use beadroll_core::journal::{self, Problem};
use beadroll_core::projection::{Extended, Projection, Record, decode_state, project};
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_git::IoError;

use crate::fs::write_atomic;
use crate::journal::{
    ViewError, claim_files, folder_name, load_journal, load_listed, read_attempt, segment_files,
};
use crate::memo::FileKey;
use crate::workspace::Workspace;

use file::{Held, Made};

/// Where a store keeps its projection.
pub fn projection_path(ws: &Workspace) -> PathBuf {
    ws.dir.join("projection")
}

/// The projection of the store's journal as it is now: the stored one, the records appended since
/// applied after it, or the journal replayed.
pub fn projection(ws: &Workspace) -> Result<Projection, ViewError> {
    match current(ws, false)? {
        Now::Projection(projection) => Ok(*projection),
        // a state is read alone only when it is asked for alone
        Now::State(_) => Ok(project(load_journal(ws)?)?),
    }
}

/// The state of the store's journal as it is now: what [`projection`] holds, read alone when the
/// stored projection is the journal's.
pub(crate) fn state(ws: &Workspace) -> Result<State, ViewError> {
    Ok(match current(ws, true)? {
        Now::State(state) => *state,
        Now::Projection(projection) => projection.into_state(),
    })
}

/// What a command takes from the store: a state alone, or a projection.
enum Now {
    State(Box<State>),
    Projection(Box<Projection>),
}

fn current(ws: &Workspace, alone: bool) -> Result<Now, ViewError> {
    let segments = segment_files(ws)?;
    ws.settings.failpoints.hit("journal.after-enumerate");
    let Some(build) = ws.settings.build.as_deref() else {
        let journal = load_listed(ws, &segments)?.journal;
        return Ok(if alone {
            Now::State(Box::new(replay(journal)?))
        } else {
            Now::Projection(Box::new(project(journal)?))
        });
    };
    let seen = held(ws, &segments, true);
    let stored = file::read(&projection_path(ws)).filter(|stored| stored.made.build == build);
    if let (Some(stored), Some(seen)) = (stored, &seen) {
        let attempts = claim_files(ws)?;
        let listed = held(ws, &attempts, false);
        if listed
            .as_ref()
            .is_some_and(|listed| same(&stored.made, seen, listed))
        {
            if alone && let Ok(state) = decode_state(&stored.state) {
                return Ok(Now::State(Box::new(state)));
            }
            if let Some(projection) = stored.encoded().and_then(|e| Projection::decode(&e).ok()) {
                return Ok(Now::Projection(Box::new(projection)));
            }
        } else if let Some(tails) = grown(&stored.made, seen) {
            let made = stored.made.clone();
            let decoded = stored.encoded().and_then(|e| Projection::decode(&e).ok());
            if let Some(projection) = decoded
                && let Some((records, now)) = appended(ws, &made, &tails, build)?
                && let Extended::Applied(projection) = projection.extend(records)?
            {
                save(ws, &now, &projection);
                return Ok(Now::Projection(projection));
            }
        }
    }
    let loaded = load_listed(ws, &segments)?;
    let projection = project(loaded.journal)?;
    // the files read as they were listed, before and after: the projection is theirs
    let now = (seen.filter(|_| loaded.whole))
        .filter(|seen| held(ws, &segments, true).as_ref() == Some(seen))
        .zip(held(ws, &loaded.attempts, false))
        .map(|(mut files, attempts)| {
            files.extend(attempts);
            Made {
                build: build.to_owned(),
                files,
            }
        });
    if let Some(now) = now {
        save(ws, &now, &projection);
    }
    Ok(Now::Projection(Box::new(projection)))
}

/// The identities of `paths`, by their paths under the store's `events`; `None` when one cannot be
/// had (a file gone, a name that is not UTF-8): nothing is then kept of them.
fn held(ws: &Workspace, paths: &[PathBuf], segment: bool) -> Option<Vec<Held>> {
    (paths.iter())
        .map(|path| {
            let key = FileKey::of(path)?;
            let path = path.strip_prefix(&ws.events_dir).ok()?.to_str()?.to_owned();
            Some(Held { path, key, segment })
        })
        .collect()
}

/// Whether the projection was made from just these segments and attempt files, as they are.
fn same(made: &Made, segments: &[Held], attempts: &[Held]) -> bool {
    let files: HashMap<&str, &Held> = (made.files.iter())
        .map(|held| (held.path.as_str(), held))
        .collect();
    files.len() == segments.len() + attempts.len()
        && (segments.iter().chain(attempts))
            .all(|held| files.get(held.path.as_str()) == Some(&held))
}

/// A segment to read from `from`: one that grew since the projection was made (from where it was
/// read up to), or a new one (from its start).
struct Tail {
    path: String,
    from: u64,
}

/// What of the segments grew since the projection was made, when that is all that happened to
/// them: each it was made from is the same file, as long as it was or longer, and a new one comes
/// after a machine's others. `None` otherwise: a file was replaced, rewritten in place, cut or
/// removed, or a machine's journal began.
fn grown(made: &Made, segments: &[Held]) -> Option<Vec<Tail>> {
    let before: HashMap<&str, &FileKey> = (made.files.iter())
        .filter(|held| held.segment)
        .map(|held| (held.path.as_str(), &held.key))
        .collect();
    // each machine's last segment, by name
    let mut last: HashMap<&str, &str> = HashMap::new();
    for held in made.files.iter().filter(|held| held.segment) {
        let was = last.entry(machine(&held.path)).or_insert(&held.path);
        *was = (*was).max(held.path.as_str());
    }
    let mut tails = Vec::new();
    for held in segments {
        match before.get(held.path.as_str()) {
            Some(was) if held.key == **was => {}
            Some(was) if held.key.same_file(was) && held.key.len() > was.len() => {
                tails.push(Tail {
                    path: held.path.clone(),
                    from: was.len(),
                })
            }
            Some(_) => return None,
            None => {
                let after = (last.get(machine(&held.path)))?;
                if held.path.as_str() <= *after {
                    return None;
                }
                tails.push(Tail {
                    path: held.path.clone(),
                    from: 0,
                });
            }
        }
    }
    let still: HashSet<&str> = segments.iter().map(|held| held.path.as_str()).collect();
    before
        .keys()
        .all(|path| still.contains(path))
        .then_some(tails)
}

/// The machine whose folder holds the file at `path` (`<machine>/...`).
fn machine(path: &str) -> &str {
    path.split('/').next().unwrap_or_default()
}

/// The records appended since the projection was made: each grown segment's from where it was
/// read up to, then each attempt file it was not made from, listed once the segments are read;
/// and what a projection with them is made from. `None` when what was appended cannot be read as
/// a store reads it whole: a segment ends in a line not yet whole, a record is unreadable, an
/// attempt file was replaced or removed.
fn appended(
    ws: &Workspace,
    made: &Made,
    tails: &[Tail],
    build: &str,
) -> Result<Option<(Vec<Record>, Made)>, ViewError> {
    let mut records = Vec::new();
    let mut problems: Vec<Problem> = Vec::new();
    let mut read: HashMap<&str, FileKey> = HashMap::new();
    for tail in tails {
        let path = ws.events_dir.join(&tail.path);
        let Some((data, key)) = read_from(&path, tail.from)? else {
            return Ok(None);
        };
        if data.last().is_some_and(|last| *last != b'\n') {
            return Ok(None);
        }
        let origin = path.display().to_string();
        let replica = folder_name(&path, 1);
        for value in journal::read_segment(&data, &origin, Some(&mut problems)) {
            records.push(Record {
                value,
                replica: replica.clone(),
                origin: origin.clone(),
                sequenced: true,
            });
        }
        read.insert(&tail.path, key);
    }
    let attempts = claim_files(ws)?;
    let Some(listed) = held(ws, &attempts, false) else {
        return Ok(None);
    };
    let before: HashMap<&str, &FileKey> = (made.files.iter())
        .filter(|held| !held.segment)
        .map(|held| (held.path.as_str(), &held.key))
        .collect();
    let still: HashSet<&str> = listed.iter().map(|held| held.path.as_str()).collect();
    if !before.keys().all(|path| still.contains(path)) {
        return Ok(None); // an attempt file is gone
    }
    for (path, held) in attempts.iter().zip(&listed) {
        match before.get(held.path.as_str()) {
            Some(was) if **was == held.key => continue,
            Some(_) => return Ok(None),
            None => {}
        }
        let origin = path.display().to_string();
        let replica = folder_name(path, 2);
        for value in read_attempt(path, Some(&mut problems)) {
            records.push(Record {
                value,
                replica: replica.clone(),
                origin: origin.clone(),
                sequenced: false,
            });
        }
    }
    if !problems.is_empty() {
        return Ok(None);
    }
    let mut files: Vec<Held> = Vec::new();
    for held in made.files.iter().filter(|held| held.segment) {
        let key = read.remove(held.path.as_str()).unwrap_or(held.key);
        files.push(Held {
            path: held.path.clone(),
            key,
            segment: true,
        });
    }
    for tail in tails.iter().filter(|tail| tail.from == 0) {
        if let Some(key) = read.remove(tail.path.as_str()) {
            files.push(Held {
                path: tail.path.clone(),
                key,
                segment: true,
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.extend(listed);
    let now = Made {
        build: build.to_owned(),
        files,
    };
    Ok(Some((records, now)))
}

/// The bytes of the file at `path` from `from` up to its length when it was opened, and its
/// identity up to there; `None` when it is not there.
fn read_from(path: &Path, from: u64) -> Result<Option<(Vec<u8>, FileKey)>, ViewError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(crate::Error::Io(IoError::at(&error, path)).into()),
    };
    let meta = file
        .metadata()
        .map_err(|e| crate::Error::Io(IoError::at(&e, path)))?;
    let Some(key) = FileKey::of_meta(&meta).filter(|key| key.len() >= from) else {
        return Ok(None);
    };
    let mut data = Vec::new();
    file.seek(SeekFrom::Start(from))
        .and_then(|_| (&mut file).take(key.len() - from).read_to_end(&mut data))
        .map_err(|e| crate::Error::Io(IoError::at(&e, path)))?;
    let read = from + data.len() as u64;
    Ok(Some((data, key.up_to(read))))
}

/// Writes `projection` back as made from `made`, when the store is there; a cache that cannot be
/// written is no failure.
fn save(ws: &Workspace, made: &Made, projection: &Projection) {
    if made.files.is_empty() || !ws.events_dir.is_dir() {
        return;
    }
    let bytes = file::bytes(made, &projection.encode());
    let _ = write_atomic(&projection_path(ws), &bytes, true, &ws.settings.failpoints);
}

#[cfg(test)]
mod tests;
