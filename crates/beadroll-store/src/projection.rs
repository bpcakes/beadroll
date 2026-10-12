//! The store's projection (roll-lqjj.2, step 3 of epic roll-lqjj): the state its journal replays
//! into, kept in `<store>/projection` with what it was made from (the build of bead that made it,
//! every journal file's identity on disk as the memo of roll-lqjj.1 compares files, and a sum of
//! each segment's bytes), so that a command reads it instead of replaying the whole journal. It is
//! a cache, never a source.
//!
//! A command lists the journal's files as replay does and compares them with the ones the
//! projection names. All the same: the projection is read, a command that only reads reading its
//! state alone. Records appended since (a segment grown whose bytes before are the ones summed, a
//! segment or an attempt file added) are read alone and applied after it, when replay would apply
//! them so ([`Projection::extend`]). Anything else (a file replaced, rewritten, cut or gone, a
//! record replay would apply otherwise) and a projection that is missing, unreadable, damaged or
//! written by another build of bead: the journal is replayed. A projection made is written back,
//! whole or not at all, with the identity each file had as it was read, when every segment ended
//! with a whole line, never into a store that is not there. Deleting it is safe at any moment.

mod file;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use beadroll_core::journal::{self, Problem};
use beadroll_core::projection::{Extended, Projection, Record, decode_state, project};
use beadroll_core::replay::replay;
use beadroll_core::state::State;

use crate::fs::{content_sum, read_keyed, write_atomic};
use crate::journal::{
    ViewError, attempt_events, claim_files, folder_name, load_journal, load_listed, segment_files,
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
        let journal = load_listed(ws, &segments, false)?.journal;
        return Ok(if alone {
            Now::State(Box::new(replay(journal)?))
        } else {
            Now::Projection(Box::new(project(journal)?))
        });
    };
    let stored = file::read(&projection_path(ws)).filter(|stored| stored.made.build == build);
    if let (Some(stored), Some(seen)) = (stored, held(ws, &segments, true)) {
        let attempts = claim_files(ws)?;
        let listed = held(ws, &attempts, false);
        if listed
            .as_ref()
            .is_some_and(|listed| same(&stored.made, &seen, listed))
        {
            if alone && let Ok(state) = decode_state(&stored.state) {
                return Ok(Now::State(Box::new(state)));
            }
            if let Some(projection) = stored.encoded().and_then(|e| Projection::decode(&e).ok()) {
                return Ok(Now::Projection(Box::new(projection)));
            }
        } else if let Some(tails) = grown(&stored.made, &seen) {
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
    let loaded = load_listed(ws, &segments, true)?;
    let projection = project(loaded.journal)?;
    // each file as it was read: the projection is theirs
    let read = (segments.iter().zip(loaded.segments))
        .map(|(path, (key, sum))| kept(ws, path, key, true, sum))
        .chain(
            loaded
                .attempts
                .iter()
                .map(|(path, key)| kept(ws, path, *key, false, None)),
        )
        .collect::<Option<Vec<Held>>>();
    if let Some(files) = read.filter(|_| loaded.whole) {
        let build = build.to_owned();
        save(ws, &Made { build, files }, &projection);
    }
    Ok(Now::Projection(Box::new(projection)))
}

/// The identities of `paths` now, by their paths under the store's `events`; `None` when one
/// cannot be had (a file gone, a name that is not UTF-8): nothing is then kept of them.
fn held(ws: &Workspace, paths: &[PathBuf], segment: bool) -> Option<Vec<Held>> {
    (paths.iter())
        .map(|path| kept(ws, path, FileKey::of(path), segment, None))
        .collect()
}

/// What a projection keeps of the file at `path`: its path under the store's `events`, its
/// identity `key`, whether it is a segment, and the sum of its bytes when they were read; `None`
/// without an identity or a UTF-8 path.
fn kept(
    ws: &Workspace,
    path: &Path,
    key: Option<FileKey>,
    segment: bool,
    sum: Option<[u8; 16]>,
) -> Option<Held> {
    let path = path.strip_prefix(&ws.events_dir).ok()?.to_str()?.to_owned();
    Some(Held {
        path,
        key: key?,
        segment,
        sum,
    })
}

/// Whether the projection was made from just these segments and attempt files, as they are: the
/// same paths and identities.
fn same(made: &Made, segments: &[Held], attempts: &[Held]) -> bool {
    let files: HashMap<&str, &Held> = (made.files.iter())
        .map(|held| (held.path.as_str(), held))
        .collect();
    files.len() == segments.len() + attempts.len()
        && (segments.iter().chain(attempts)).all(|now| {
            (files.get(now.path.as_str()))
                .is_some_and(|was| was.key == now.key && was.segment == now.segment)
        })
}

/// A segment to read: one that grew since the projection was made, whose bytes up to `from` were
/// summed as `sum`, or a new one (`from` 0).
struct Tail {
    path: String,
    from: u64,
    sum: Option<[u8; 16]>,
}

/// What of the segments grew since the projection was made, when that is all that happened to
/// them: each it was made from is the same file, as long as it was or longer, and a new one comes
/// after a machine's others. `None` otherwise: a file was replaced, rewritten in place, cut or
/// removed, or a machine's journal began. Whether a grown segment kept its bytes is for its sum to
/// say, once it is read ([`appended`]).
fn grown(made: &Made, segments: &[Held]) -> Option<Vec<Tail>> {
    let before: HashMap<&str, &Held> = (made.files.iter())
        .filter(|held| held.segment)
        .map(|held| (held.path.as_str(), held))
        .collect();
    // each machine's last segment, by name
    let mut last: HashMap<&str, &str> = HashMap::new();
    for held in made.files.iter().filter(|held| held.segment) {
        let was = last.entry(machine(&held.path)).or_insert(&held.path);
        *was = (*was).max(held.path.as_str());
    }
    let mut tails = Vec::new();
    for now in segments {
        match before.get(now.path.as_str()) {
            Some(was) if now.key == was.key => {}
            Some(was) if now.key.same_file(&was.key) && now.key.len() > was.key.len() => {
                tails.push(Tail {
                    path: now.path.clone(),
                    from: was.key.len(),
                    sum: was.sum,
                });
            }
            Some(_) => return None,
            None => {
                let after = (last.get(machine(&now.path)))?;
                if now.path.as_str() <= *after {
                    return None;
                }
                tails.push(Tail {
                    path: now.path.clone(),
                    from: 0,
                    sum: None,
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

/// The records appended since the projection was made: each grown segment's after where it was
/// read up to, then each attempt file it was not made from, listed once the segments are read;
/// and what a projection with them is made from, each file as it was read. `None` when what was
/// appended cannot be read as a store reads it whole: a grown segment's bytes before are not the
/// ones summed, a segment ends in a line not yet whole, a record is unreadable, an attempt file was
/// replaced or removed.
fn appended(
    ws: &Workspace,
    made: &Made,
    tails: &[Tail],
    build: &str,
) -> Result<Option<(Vec<Record>, Made)>, ViewError> {
    let mut records = Vec::new();
    let mut problems: Vec<Problem> = Vec::new();
    let mut read: HashMap<&str, Held> = HashMap::new();
    for tail in tails {
        let path = ws.events_dir.join(&tail.path);
        let Ok((data, Some(key))) = read_keyed(&path) else {
            return Ok(None);
        };
        let from = usize::try_from(tail.from).unwrap_or(usize::MAX);
        let (Some(before), Some(after)) = (data.get(..from), data.get(from..)) else {
            return Ok(None);
        };
        if tail.sum.is_some_and(|sum| content_sum(before) != sum)
            || after.last().is_some_and(|last| *last != b'\n')
        {
            return Ok(None);
        }
        let origin = path.display().to_string();
        let replica = folder_name(&path, 1);
        for value in journal::read_segment(after, &origin, Some(&mut problems)) {
            records.push(Record {
                value,
                replica: replica.clone(),
                origin: origin.clone(),
                sequenced: true,
            });
        }
        let sum = Some(content_sum(&data));
        let Some(held) = kept(ws, &path, Some(key), true, sum) else {
            return Ok(None);
        };
        read.insert(&tail.path, held);
    }
    let before: HashMap<&str, &FileKey> = (made.files.iter())
        .filter(|held| !held.segment)
        .map(|held| (held.path.as_str(), &held.key))
        .collect();
    let attempts = claim_files(ws)?;
    let Some(listed) = held(ws, &attempts, false) else {
        return Ok(None);
    };
    let still: HashSet<&str> = listed.iter().map(|held| held.path.as_str()).collect();
    if !before.keys().all(|path| still.contains(path)) {
        return Ok(None); // an attempt file is gone
    }
    let mut files: Vec<Held> = Vec::new();
    for (path, now) in attempts.iter().zip(listed) {
        match before.get(now.path.as_str()) {
            Some(was) if **was == now.key => {
                files.push(now);
                continue;
            }
            Some(_) => return Ok(None),
            None => {}
        }
        let Ok((data, Some(key))) = read_keyed(path) else {
            return Ok(None);
        };
        let origin = path.display().to_string();
        let replica = folder_name(path, 2);
        for value in attempt_events(path, Some(&data), Some(&mut problems)) {
            records.push(Record {
                value,
                replica: replica.clone(),
                origin: origin.clone(),
                sequenced: false,
            });
        }
        files.extend(kept(ws, path, Some(key), false, None));
    }
    if !problems.is_empty() {
        return Ok(None);
    }
    let mut segments: Vec<Held> = (made.files.iter())
        .filter(|held| held.segment)
        .map(|held| {
            read.remove(held.path.as_str())
                .unwrap_or_else(|| held.clone())
        })
        .collect();
    segments.extend(read.into_values());
    segments.sort_by(|a, b| a.path.cmp(&b.path));
    segments.extend(files);
    let build = build.to_owned();
    Ok(Some((
        records,
        Made {
            build,
            files: segments,
        },
    )))
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
