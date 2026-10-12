//! The journals on disk (Python's segment and attempt functions in `machine.py` and the loaders in
//! `model.py`): this machine's own append-only segments, the attempt files every machine reads,
//! this machine's pending attempts, and the state they replay into.

use std::borrow::Borrow;
use std::fs::OpenOptions;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use beadroll_core::journal::{self, Journal, Problem};
use beadroll_core::json::{self, Format, Value};
use beadroll_core::state::{KeySpace, State};
use beadroll_core::text::py_int;
use beadroll_git::IoError;

use crate::error::Error;
use crate::failpoint::{Failpoints, Fired, kill_self};
use crate::fs::{
    content_sum, dir_entries, fsync, glob_ending, has_suffix, private_dirs, read, read_keyed,
    write_atomic,
};
use crate::memo::FileKey;
use crate::workspace::Workspace;

/// Every segment of every journal in the store, journal by journal, in name order.
pub fn segment_files(ws: &Workspace) -> Result<Vec<PathBuf>, Error> {
    let mut out = Vec::new();
    for folder in dir_entries(&ws.events_dir)? {
        if folder.is_dir() {
            out.extend(
                dir_entries(&folder)?
                    .into_iter()
                    .filter(|p| has_suffix(p, ".jsonl")),
            );
        }
    }
    Ok(out)
}

/// Every accepted attempt file in the store (`events/<machine>/claims/<id>.json`).
pub fn claim_files(ws: &Workspace) -> Result<Vec<PathBuf>, Error> {
    let mut out = Vec::new();
    for folder in dir_entries(&ws.events_dir)? {
        if folder.is_dir() {
            out.extend(
                dir_entries(&folder.join("claims"))?
                    .into_iter()
                    .filter(|p| has_suffix(p, ".json")),
            );
        }
    }
    Ok(out)
}

/// This machine's own ownership attempts the remote has not confirmed. Never read as events.
pub fn pending_files(ws: &Workspace) -> Vec<PathBuf> {
    glob_ending(&ws.dir.join("pending"), ".json")
}

/// Where an attempt lives once it counts: one file, named after its first event, holding every
/// event that has to land with it.
pub fn attempt_path(ws: &Workspace, events: &[Value]) -> PathBuf {
    let first = events.first();
    let member = |key: &str| {
        first
            .and_then(|e| e.get(key))
            .map(beadroll_core::text::str_of)
            .unwrap_or_default()
    };
    ws.events_dir
        .join(member("replica"))
        .join("claims")
        .join(format!("{}.json", member("id")))
}

/// Events as journal lines: compact JSON, raw UTF-8, one per line. Events given by value are let go
/// of as each is encoded, so that their lines and they are not all held at once (roll-lqjj.9).
pub fn encode_events(
    events: impl IntoIterator<Item = impl Borrow<Value>>,
) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    for event in events {
        let line = json::write(event.borrow(), Format::COMPACT)
            .map_err(|e| Error::Validation(e.to_string()))?;
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
    }
    Ok(out)
}

/// The bytes up to the last complete record. A final record that only lacks its newline is kept,
/// read as strictly as every other record.
pub fn complete_prefix(data: &[u8]) -> Vec<u8> {
    if data.is_empty() || data.ends_with(b"\n") {
        return data.to_vec();
    }
    let cut = data
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |at| at + 1);
    if json::parse(&data[cut..]).is_ok() {
        let mut out = data.to_vec();
        out.push(b'\n');
        out
    } else {
        data[..cut].to_vec()
    }
}

/// Drops an interrupted final write, kept beside the segment as `NAME.torn-<ns>` for diagnosis, so
/// that later appends stay readable. A segment that is empty or ends with a newline is whole, and
/// only its last byte is read: a sync repairs every segment, and their length is the history's.
pub fn repair_tail(ws: &Workspace, path: &Path) -> Result<(), Error> {
    if ends_whole(path)? {
        return Ok(());
    }
    let data = read(path)?;
    let fixed = complete_prefix(&data);
    if fixed == data {
        return Ok(());
    }
    if !fixed.starts_with(&data) {
        // an incomplete fragment, not just a missing newline
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let torn = path.with_file_name(format!("{name}.torn-{}", ws.settings.clock.now_ns()));
        write_atomic(&torn, &data[fixed.len()..], true, &ws.settings.failpoints)?;
    }
    write_atomic(path, &fixed, true, &ws.settings.failpoints)
}

/// Whether the file at `path` is empty or ends with a newline, from its last byte alone; a file
/// that cannot be opened is the IO error naming it, as reading it whole would be, and anything
/// but a regular file is left to that read.
fn ends_whole(path: &Path) -> Result<bool, Error> {
    let mut file = std::fs::File::open(path).map_err(|e| IoError::at(&e, path))?;
    let meta = file.metadata().map_err(|e| IoError::at(&e, path))?;
    if !meta.is_file() {
        return Ok(false);
    }
    let len = meta.len();
    if len == 0 {
        return Ok(true);
    }
    let mut last = [0u8];
    file.seek(SeekFrom::Start(len - 1))
        .and_then(|_| file.read_exact(&mut last))
        .map_err(|e| IoError::at(&e, path))?;
    Ok(last == *b"\n")
}

/// Where this machine appends next: the last segment, or a new one once that has reached the
/// rotation size. A full segment is never touched again. A last segment whose name is not a number
/// is an IO error, where Python dies with a traceback (`docs/RUST-PORT.md` §5 row 26).
pub fn active_segment(ws: &Workspace, replica: &str) -> Result<PathBuf, Error> {
    let folder = ws.events_dir.join(replica);
    let segments = glob_ending(&folder, ".jsonl");
    let Some(last) = segments.last() else {
        return Ok(folder.join("000001.jsonl"));
    };
    let size = std::fs::metadata(last)
        .map_err(|e| IoError::at(&e, last))?
        .len();
    if i128::from(size) < ws.settings.rotate_bytes {
        return Ok(last.clone());
    }
    let name = last.file_name().unwrap_or_default().to_string_lossy();
    let stem = name.strip_suffix(".jsonl").unwrap_or(&name);
    let unnumbered = || {
        Error::io_message(format!(
            "{} is not a numbered journal segment; move it out of {}",
            last.display(),
            folder.display()
        ))
    };
    let next = py_int(stem).ok_or_else(unnumbered)?.plus_one();
    if next.digit_count() > beadroll_core::json::MAX_DIGITS {
        return Err(unnumbered());
    }
    Ok(folder.join(format!("{}.jsonl", next.zero_padded(6))))
}

/// Appends sequential events to this machine's journal, after repairing any torn tail. Called with
/// the store lock held. With the `append.mid` failpoint set, the first half of the line is written
/// and flushed before it is passed (`torn-sigkill` fsyncs that half and dies there).
pub fn append_events(
    ws: &Workspace,
    replica: &str,
    events: impl IntoIterator<Item = impl Borrow<Value>>,
) -> Result<(), Error> {
    let folder = ws.events_dir.join(replica);
    for segment in glob_ending(&folder, ".jsonl") {
        repair_tail(ws, &segment)?;
    }
    let path = active_segment(ws, replica)?;
    if let Some(parent) = path.parent() {
        private_dirs(parent)?;
    }
    let data = encode_events(events)?;
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| IoError::at(&e, &path))?;
    // at the end before the first write, where CPython's append mode puts it: a file-size limit is
    // then checked against the end of the file, which Darwin reads from the descriptor's offset
    file.seek(SeekFrom::End(0)).map_err(|e| IoError::bare(&e))?;
    let failpoints = &ws.settings.failpoints;
    let mut rest: &[u8] = &data;
    if failpoints.has("append.mid") {
        let (half, after) = data.split_at(data.len() / 2);
        file.write_all(half).map_err(|e| IoError::bare(&e))?;
        file.flush().map_err(|e| IoError::bare(&e))?;
        if failpoints.hit("append.mid") == Some(Fired::TornSigkill) {
            fsync(&file)?;
            kill_self();
        }
        rest = after;
    }
    file.write_all(rest).map_err(|e| IoError::bare(&e))?;
    fsync(&file)?;
    Ok(())
}

/// Appends `more` to the segment at `path`, which is there: what another machine's published copy
/// has beyond ours, at a merge (roll-lqjj.2). Like bead's own records it is appended, never
/// rewritten whole, so that the segment stays the file a projection was made from, and a
/// projection takes in these lines alone. A reader of a segment skips a last line not yet whole,
/// and a merge cuts one an interrupted append left before it reads ([`repair_tail`]).
pub fn extend_segment(path: &Path, more: &[u8]) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| IoError::at(&e, path))?;
    file.seek(SeekFrom::End(0)).map_err(|e| IoError::bare(&e))?;
    file.write_all(more).map_err(|e| IoError::bare(&e))?;
    fsync(&file)?;
    Ok(())
}

/// Writes an attempt whole: a reader sees all of it or none of it.
pub fn write_attempt_file(
    path: &Path,
    events: &[Value],
    failpoints: &Failpoints,
) -> Result<(), Error> {
    write_atomic(path, &encode_events(events)?, true, failpoints)
}

/// The events of the attempt file at `path`, as replay reads them: an unreadable file, or any bad
/// line, empties it, reported in `problems`.
pub fn read_attempt(path: &Path, problems: Option<&mut Vec<Problem>>) -> Vec<Value> {
    attempt_events(path, std::fs::read(path).ok().as_deref(), problems)
}

/// The events of the attempt file at `path` whose bytes are `data` (`None`: it could not be read),
/// as [`read_attempt`] reads them.
pub(crate) fn attempt_events(
    path: &Path,
    data: Option<&[u8]>,
    problems: Option<&mut Vec<Problem>>,
) -> Vec<Value> {
    let origin = path.display().to_string();
    match data {
        Some(data) => journal::read_attempt(data, &origin, problems),
        None => {
            if let Some(problems) = problems {
                problems.push(Problem {
                    event: None,
                    origin,
                    problem: "attempt file is unreadable".to_owned(),
                });
            }
            Vec::new()
        }
    }
}

/// Every event the store holds: the segments, then the accepted attempt files, settled.
pub fn load_journal(ws: &Workspace) -> Result<Journal, Error> {
    let segments = segment_files(ws)?;
    ws.settings.failpoints.hit("journal.after-enumerate");
    Ok(load_listed(ws, &segments, false)?.journal)
}

/// What [`load_listed`] read.
pub(crate) struct Loaded {
    /// The journal, settled.
    pub(crate) journal: Journal,
    /// Whether every segment ended with a whole line.
    pub(crate) whole: bool,
    /// Each segment's identity as it was read, and the sum of its bytes when they were summed.
    pub(crate) segments: Vec<(Option<FileKey>, Option<[u8; 16]>)>,
    /// The accepted attempt files, listed once the segments were read, each with its identity as
    /// it was read (`None` when it could not be).
    pub(crate) attempts: Vec<(PathBuf, Option<FileKey>)>,
}

/// The journal of `segments`, then of the accepted attempt files, listed once the segments are
/// read: an attempt file is in place before any record built on it is appended. Each file's
/// identity is the one it had as it was read; with `summed`, so is the sum of each segment's bytes.
pub(crate) fn load_listed(
    ws: &Workspace,
    segments: &[PathBuf],
    summed: bool,
) -> Result<Loaded, Error> {
    let mut journal = Journal::new();
    let mut whole = true;
    let mut held = Vec::with_capacity(segments.len());
    for path in segments {
        let (data, key) = read_keyed(path)?;
        whole &= data.last().is_none_or(|last| *last == b'\n');
        held.push((key, summed.then(|| content_sum(&data))));
        let origin = path.display().to_string();
        let replica = folder_name(path, 1);
        let events = journal::read_segment(&data, &origin, Some(&mut journal.invalid));
        for event in events {
            journal.add(event, &replica, &origin, true);
        }
    }
    let mut attempts = Vec::new();
    for path in claim_files(ws)? {
        let origin = path.display().to_string();
        let replica = folder_name(&path, 2);
        let read = read_keyed(&path).ok();
        let data = read.as_ref().map(|(data, _)| data.as_slice());
        for event in attempt_events(&path, data, Some(&mut journal.invalid)) {
            journal.add(event, &replica, &origin, false);
        }
        attempts.push((path, read.and_then(|(_, key)| key)));
    }
    journal.settle();
    Ok(Loaded {
        journal,
        whole,
        segments: held,
        attempts,
    })
}

/// The name of `path`'s ancestor `up` levels above it (1: its folder).
pub(crate) fn folder_name(path: &Path, up: usize) -> String {
    let mut at = path;
    for _ in 0..up {
        at = at.parent().unwrap_or(at);
    }
    at.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// This machine's unconfirmed attempts, each a list of events. Only the holder of the store lock
/// sweeps: a pending file it has read and found not to be an attempt is nothing to settle, and is
/// removed with its receipt. A file that cannot be read is not junk (it may be an accepted attempt
/// whose receipt a retry needs): under the lock that is an IO error, and a reader skips it, as it
/// skips a file that vanished (settled meanwhile by the lock holder, which keeps the receipt).
pub fn load_pending(ws: &Workspace) -> Result<Vec<Vec<Value>>, Error> {
    let mut out = Vec::new();
    let listed = pending_files(ws);
    ws.settings.failpoints.hit("pending.after-list");
    for path in listed {
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) if ws.holding() => return Err(IoError::at(&error, &path).into()),
            Err(_) => continue,
        };
        let events = journal::read_attempt(&data, &path.display().to_string(), None);
        if !events.is_empty() {
            out.push(events);
        } else if ws.holding() {
            remove(&path)?;
            remove(&path.with_extension("command"))?;
        }
    }
    Ok(out)
}

/// Removes `path`; already gone is fine (Python's `unlink(missing_ok=True)`), any other failure is
/// an IO error naming it.
pub fn remove(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(IoError::at(&error, path).into()),
    }
}

/// Why the store's state could not be had: a file could not be read, or a renamed copy found every
/// free key taken.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ViewError {
    /// The store could not be read.
    #[error("{0}")]
    Store(#[from] Error),
    /// Replay ran out of keys (`KEY_SPACE`).
    #[error("{0}")]
    KeySpace(#[from] KeySpace),
}

/// Current state as this machine should see it: confirmed events, with this machine's own
/// unconfirmed ownership attempts listed on their issues but counting for nothing.
pub fn view(ws: &Workspace) -> Result<State, ViewError> {
    let state = crate::projection::state(ws)?;
    pending_listed(ws, state)
}

/// [`view`], its issues' texts there (roll-lqjj.6): for a command that prints none of them, or
/// reads in those it does (`read_texts`).
pub fn view_outline(ws: &Workspace) -> Result<State, ViewError> {
    let state = crate::projection::outline(ws)?;
    pending_listed(ws, state)
}

/// `state` with this machine's own unconfirmed ownership attempts listed on their issues.
fn pending_listed(ws: &Workspace, mut state: State) -> Result<State, ViewError> {
    ws.settings.failpoints.hit("view.before-load-pending");
    for attempt in load_pending(ws)? {
        for event in &attempt {
            state.add_unconfirmed(event);
        }
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    use beadroll_core::json::Object;

    /// Events given by value, to be let go of as they are encoded (roll-lqjj.9), make the lines
    /// the same events given by reference make.
    #[test]
    fn events_encode_alike_given_by_value_or_by_reference() {
        let events: Vec<Value> = (1..=3)
            .map(|n| {
                let mut event = Object::new();
                event.insert("v".into(), Value::from(1));
                event.insert("op".into(), Value::from("comment"));
                event.insert("text".into(), Value::from(format!("line {n}: \u{e9}\n")));
                Value::from(event)
            })
            .collect();
        let by_reference = encode_events(&events).unwrap();
        assert_eq!(by_reference.iter().filter(|b| **b == b'\n').count(), 3);
        assert_eq!(encode_events(events).unwrap(), by_reference);
    }

    /// A segment is whole when it is empty or ends with a newline, which its last byte says; any
    /// other file is read whole, a directory included, and one that is not there is the IO error
    /// naming it, as that read's is.
    #[test]
    fn a_segment_is_whole_by_its_last_byte() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, data: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, data).unwrap();
            path
        };
        assert!(ends_whole(&file("empty", b"")).unwrap());
        assert!(ends_whole(&file("whole", b"{}\n{}\n")).unwrap());
        assert!(!ends_whole(&file("torn", b"{}\n{\"v\"")).unwrap());
        assert!(!ends_whole(&file("unended", b"{}")).unwrap());
        let folder = dir.path().join("000001.jsonl");
        std::fs::create_dir(&folder).unwrap();
        assert!(!ends_whole(&folder).unwrap(), "left to the whole read");
        let missing = dir.path().join("missing");
        let read = std::fs::read(&missing).unwrap_err();
        assert_eq!(
            ends_whole(&missing).unwrap_err(),
            Error::Io(IoError::at(&read, &missing))
        );
    }
}
