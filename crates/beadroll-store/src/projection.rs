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
//! them so ([`Projection::extend`]). A file whose times are the projection's own, which could
//! have been rewritten in place since with nothing to show for it, is read again and its bytes
//! summed: the sum kept of it says whether it is the one the projection was made from. Anything
//! else (a file replaced, rewritten, cut or gone, a record replay would apply otherwise) and a
//! projection that is missing, unreadable, damaged or written by another build of bead: the
//! journal is replayed. A projection made is written back, whole or not at all, with the identity
//! each file had as it was read, when every segment ended with a whole line, never into a store
//! that is not there. Deleting it is safe at any moment, and so it is not flushed: whatever a crash
//! leaves of it, or of its text file, is read as replay would answer or not at all
//! ([`crate::fs::write_cache_parts`]).
//!
//! Each issue's texts are kept apart, in `<store>/projection-texts` (roll-lqjj.6, [`texts`]): a
//! state read from the projection has them there, and a command reads in those it prints. A
//! command that prints none (`list`, `ready`) reads the state alone ([`outline`]); one that may
//! print any, every text ([`state`]); and records applied after the projection have the texts of
//! the issues they are about read in first. Texts that cannot be read as written have the journal
//! replayed, as a projection that cannot be does. Texts written nowhere yet are appended to the
//! text file, and a projection made whole, or whose text file holds more texts no longer read than
//! read, writes them all to a new one.

mod file;
mod texts;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use beadroll_core::id::EventId;
use beadroll_core::journal::{self, Problem};
use beadroll_core::json::Value;
use beadroll_core::projection::{Extended, Projection, Record, decode_state, project};
use beadroll_core::replay::replay;
use beadroll_core::state::State;

use crate::fs::{content_sum, read_keyed, write_cache_parts};
use crate::journal::{
    ViewError, attempt_events, claim_files, folder_name, load_journal, load_listed, segment_files,
};
use crate::memo::FileKey;
use crate::workspace::Workspace;

use file::{Held, Made};

pub use texts::texts_path;

/// How many bytes of texts no longer read a text file may hold beyond as many as are read, before
/// a projection writes its texts to a new one.
const SLACK: u64 = 1 << 20;

/// Where a store keeps its projection.
pub fn projection_path(ws: &Workspace) -> PathBuf {
    ws.dir.join("projection")
}

/// The projection of the store's journal as it is now: the stored one, the records appended since
/// applied after it, or the journal replayed. Its issues' texts are there but for those the records
/// applied are about; [`read_texts`] reads in more.
pub fn projection(ws: &Workspace) -> Result<Projection, ViewError> {
    match current(ws, Want::Projection)? {
        Now::Projection(projection) => Ok(*projection),
        Now::State(_) => Ok(project(load_journal(ws)?)?),
    }
}

/// The state of the store's journal as it is now, every issue's texts in it: what [`projection`]
/// holds, read alone when the stored projection is the journal's.
pub(crate) fn state(ws: &Workspace) -> Result<State, ViewError> {
    Ok(match current(ws, Want::Texts)? {
        Now::State(state) => *state,
        Now::Projection(projection) => projection.into_state(),
    })
}

/// The state of the store's journal as it is now, its issues' texts there, for a command that
/// prints none of them or reads in those it does ([`read_texts`]).
pub(crate) fn outline(ws: &Workspace) -> Result<State, ViewError> {
    Ok(match current(ws, Want::Outline)? {
        Now::State(state) => *state,
        Now::Projection(projection) => projection.into_state(),
    })
}

/// Reads into `state` the texts of the issues `uids` names that it has there: `false` when they
/// cannot be read as written, its text file gone or changed, and nothing then read.
pub fn read_texts(
    ws: &Workspace,
    state: &mut State,
    uids: impl IntoIterator<Item = EventId>,
) -> bool {
    let wanted = state.wanted_texts(uids);
    read_wanted(ws, state, &wanted)
}

/// Reads into `state` the texts of every issue it has there.
fn read_all(ws: &Workspace, state: &mut State) -> bool {
    let wanted = state.all_wanted_texts();
    read_wanted(ws, state, &wanted)
}

fn read_wanted(
    ws: &Workspace,
    state: &mut State,
    wanted: &[(EventId, beadroll_core::state::TextPlace)],
) -> bool {
    if wanted.is_empty() {
        return true;
    }
    let file = state
        .texts_file()
        .and_then(|generation| texts::open(ws, generation, false));
    file.is_some_and(|file| texts::read_in(&file, state, wanted))
}

/// What a command wants of the store.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    /// A projection, to apply events to.
    Projection,
    /// A state, every text in it.
    Texts,
    /// A state, its texts there.
    Outline,
}

/// What a command takes from the store: a state alone, or a projection.
enum Now {
    State(Box<State>),
    Projection(Box<Projection>),
}

/// The issues `records` are about.
fn about(records: &[Record]) -> Vec<EventId> {
    (records.iter())
        .filter_map(|record| record.value.get("issue").and_then(Value::as_str))
        .filter_map(EventId::parse)
        .collect()
}

fn current(ws: &Workspace, want: Want) -> Result<Now, ViewError> {
    let segments = segment_files(ws)?;
    ws.settings.failpoints.hit("journal.after-enumerate");
    let Some(build) = ws.settings.build.as_deref() else {
        let journal = load_listed(ws, &segments, false)?.journal;
        return Ok(if want == Want::Projection {
            Now::Projection(Box::new(project(journal)?))
        } else {
            Now::State(Box::new(replay(journal)?))
        });
    };
    let stored = file::read(&projection_path(ws)).filter(|stored| stored.made.build == build);
    if let (Some(stored), Some(seen)) = (stored, held(ws, &segments, true)) {
        let attempts = claim_files(ws)?;
        let listed = held(ws, &attempts, false);
        let file = Some(stored.made.texts);
        let settled = stored.written;
        if listed
            .as_ref()
            .is_some_and(|listed| same(&stored.made, &seen, listed, settled))
        {
            if want != Want::Projection
                && let Ok(mut state) = decode_state(&stored.state)
            {
                state.set_texts_file(file);
                // texts that cannot be read have the journal replayed
                if want == Want::Outline || read_all(ws, &mut state) {
                    return Ok(Now::State(Box::new(state)));
                }
            } else if let Some(mut projection) =
                stored.encoded().and_then(|e| Projection::decode(&e).ok())
            {
                projection.state_mut().set_texts_file(file);
                return Ok(Now::Projection(Box::new(projection)));
            }
        } else if let Some(tails) = grown(&stored.made, &seen, settled) {
            let made = stored.made.clone();
            let decoded = stored.encoded().and_then(|e| Projection::decode(&e).ok());
            if let Some(mut projection) = decoded
                && let Some((records, now)) = appended(ws, &made, &tails, build, settled)?
            {
                projection.state_mut().set_texts_file(file);
                let uids = about(&records);
                if read_texts(ws, projection.state_mut(), uids)
                    && let Extended::Applied(mut projection) = projection.extend(records)?
                {
                    save(ws, now, &mut projection);
                    if want != Want::Texts || read_all(ws, projection.state_mut()) {
                        return Ok(Now::Projection(projection));
                    }
                }
            }
        }
    }
    let loaded = load_listed(ws, &segments, true)?;
    let mut projection = project(loaded.journal)?;
    // each file as it was read: the projection is theirs
    let read = (segments.iter().zip(loaded.segments))
        .map(|(path, (key, sum))| kept(ws, path, key, true, sum))
        .chain(
            loaded
                .attempts
                .iter()
                .map(|(path, key, sum)| kept(ws, path, *key, false, *sum)),
        )
        .collect::<Option<Vec<Held>>>();
    if let Some(files) = read.filter(|_| loaded.whole) {
        let build = build.to_owned();
        let texts = [0; 16];
        save(
            ws,
            Made {
                build,
                files,
                texts,
            },
            &mut projection,
        );
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
/// same paths and identities, each settled when the projection was written (`settled`), since an
/// identity of that moment's own tick cannot say a file was not rewritten in place since.
fn same(made: &Made, segments: &[Held], attempts: &[Held], settled: i128) -> bool {
    let files: HashMap<&str, &Held> = (made.files.iter())
        .map(|held| (held.path.as_str(), held))
        .collect();
    files.len() == segments.len() + attempts.len()
        && (segments.iter().chain(attempts)).all(|now| {
            (files.get(now.path.as_str())).is_some_and(|was| {
                was.key == now.key && was.segment == now.segment && was.key.settled_before(settled)
            })
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
/// them: each it was made from is the same file, as long as it was or longer, only a machine's
/// last one longer (records appended to an earlier one come before the later ones' in its
/// journal). A file whose identity was not settled when the projection was written (`settled`)
/// is read whole instead, however unchanged it looks, and the sum kept of it says whether it is
/// the one the projection was made from. `None` otherwise: a file was replaced, rewritten in
/// place, cut or removed, an earlier segment grew, a machine's journal began, or a file of the
/// projection's own tick has no sum to be read against. Whether a grown segment kept its bytes is
/// for its sum to say, once it is read ([`appended`]).
fn grown(made: &Made, segments: &[Held], settled: i128) -> Option<Vec<Tail>> {
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
            Some(was) if now.key == was.key && was.key.settled_before(settled) => {}
            // Nothing was appended, but the identity cannot vouch for the bytes: they must.
            Some(was) if now.key == was.key => tails.push(Tail {
                path: now.path.clone(),
                from: was.key.len(),
                sum: Some(was.sum?),
            }),
            Some(was)
                if now.key.same_file(&was.key)
                    && now.key.len() > was.key.len()
                    && last.get(machine(&now.path)) == Some(&now.path.as_str()) =>
            {
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

/// What a projection keeps of the attempt file at `path`, whose identity `was` not settled when
/// the projection was written, read against the sum kept of it: `None` when it cannot be read, is
/// another file than `now`, or holds other bytes than the ones summed. An attempt file never
/// changes but by being replaced, so there is nothing to apply after it either way.
fn resummed(ws: &Workspace, path: &Path, was: &Held, now: &Held) -> Option<Held> {
    let (data, key) = read_keyed(path).ok()?;
    let key = key.filter(|key| *key == now.key)?;
    if was.sum != Some(content_sum(&data)) {
        return None;
    }
    kept(ws, path, Some(key), false, was.sum)
}

/// The machine whose folder holds the file at `path` (`<machine>/...`).
fn machine(path: &str) -> &str {
    path.split('/').next().unwrap_or_default()
}

/// The records appended since the projection was made: each grown segment's after where it was
/// read up to, then each attempt file it was not made from, listed once the segments are read;
/// and what a projection with them is made from, each file as it was read. An attempt file whose
/// identity was not settled when the projection was written (`settled`) is read and summed, as a
/// segment of that moment's own tick is. `None` when what was appended cannot be read as a store
/// reads it whole: a grown segment's bytes before are not the ones summed, a file of the
/// projection's own tick is not the one it was made from or has no sum, a segment ends in a line
/// not yet whole, a record is unreadable, an attempt file was replaced or removed.
fn appended(
    ws: &Workspace,
    made: &Made,
    tails: &[Tail],
    build: &str,
    settled: i128,
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
    let before: HashMap<&str, &Held> = (made.files.iter())
        .filter(|held| !held.segment)
        .map(|held| (held.path.as_str(), held))
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
            Some(was) if was.key == now.key && was.key.settled_before(settled) => {
                files.push(now);
                continue;
            }
            Some(was) if was.key != now.key => return Ok(None),
            // Of the projection's own tick: only its bytes can say it is the file it was made
            // from, and an attempt file never changes but by being replaced.
            Some(was) => match resummed(ws, path, was, &now) {
                Some(held) => {
                    files.push(held);
                    continue;
                }
                None => return Ok(None),
            },
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
        files.extend(kept(ws, path, Some(key), false, Some(content_sum(&data))));
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
            texts: made.texts,
        },
    )))
}

/// Writes `projection` back as made from `made`, when the store is there; a cache that cannot be
/// written is no failure. Its texts written nowhere yet go to the end of the text file its others
/// are placed in, under that file's lock, and the projection after them; when there is no such
/// file, or more of it is no longer read than is, every text goes to a new one, when the
/// projection has them all.
fn save(ws: &Workspace, mut made: Made, projection: &mut Projection) {
    if made.files.is_empty() || !ws.events_dir.is_dir() {
        return;
    }
    let generation = projection.state().texts_file();
    if let Some(generation) = generation
        && let Some(file) = texts::open(ws, generation, true)
        && file.lock().is_ok()
        && let Ok(len) = file.metadata().map(|meta| meta.len())
    {
        let read = projection.state().placed_text_bytes();
        // more of the file no longer read than read: every text read in, to write them all again
        let anew = len > texts::START + 2 * read + SLACK && {
            let wanted = projection.state().all_wanted_texts();
            texts::read_in(&file, projection.state_mut(), &wanted)
                && projection.state_mut().write_texts_anew()
        };
        if !anew {
            let (encoded, places) = projection.encode(len);
            made.texts = generation;
            if texts::append(ws, &file, len, &encoded.texts)
                && write_cache_parts(
                    &projection_path(ws),
                    &[&file::bytes(&made, &encoded)],
                    &ws.settings.failpoints,
                )
                .is_ok()
            {
                projection.placed(&places);
            }
            return;
        }
    }
    if !projection.state_mut().write_texts_anew() {
        return; // texts placed in a text file no longer there, and not read in
    }
    let (encoded, places) = projection.encode(texts::START);
    let generation = texts::generation(&encoded.texts);
    made.texts = generation;
    if texts::start(ws, generation, &encoded.texts)
        && write_cache_parts(
            &projection_path(ws),
            &[&file::bytes(&made, &encoded)],
            &ws.settings.failpoints,
        )
        .is_ok()
    {
        projection.placed(&places);
        projection.state_mut().set_texts_file(Some(generation));
    }
}

#[cfg(test)]
mod tests;
