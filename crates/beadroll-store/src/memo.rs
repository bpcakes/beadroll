//! What the store remembers of its journal files between commands (roll-lqjj.1): for each segment
//! and attempt file, by its path in the store, the identity the file had on disk when bead last
//! read or hashed it, how many lines it held then, and the blob `sync.git` made of it. A full
//! segment is never appended to again and an attempt file never changes, so a file whose identity
//! is the remembered one need not be read or hashed again; any other file is.
//!
//! Only a file that was already settled when the memo was written is remembered: a file whose
//! times fall in the memo's own timestamp tick could have been rewritten in place since, its
//! length kept, with nothing to show for it, so the memo says nothing of it and it is read or
//! hashed again (`FileKey::settled_before`).
//!
//! The memo is a cache kept in `sync.git`, and goes with it when `sync.git` is set aside or
//! deleted. A memo that is missing, or cannot be read whole, counts as none; one that cannot be
//! written is left as it was, since nothing depends on it. Only the holder of the store lock
//! writes it, whole, under a temporary name. A remembered blob only says what `sync.git` made of a
//! file once: whoever uses one makes sure the object is still there.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use crate::FileMode;
use crate::error::Error;
use crate::fs::{read, read_keyed, write_atomic};
use crate::workspace::Workspace;

/// Which entries are kept when reading the journal memo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoRead {
    /// Whole.
    Whole,
    /// Partial.
    Partial,
}

/// The memo's first line: what it is, and the version of its layout. Each file follows as its path
/// in the store, a NUL, then its identity, its lines and its blob (`-` when not known), separated
/// by spaces and ended by a newline.
const HEADER: &[u8] = b"bead journal memo 1\n";

/// Where `ws` keeps its memo: in `sync.git`, whose objects the blobs it remembers are.
pub fn memo_path(ws: &Workspace) -> PathBuf {
    ws.dir.join("sync.git").join("journal-memo")
}

/// A file's identity on disk, as git's index compares it to tell whether a file changed: its
/// length, its modification and change times to the nanosecond, its inode and its device. Every
/// write bead makes changes one of them: an append the length, a rewrite (to a new file renamed
/// into place) the inode. A write by anything else changes the times, except while they are the
/// ones it already has: a kernel stamps an inode from a clock of its own granularity, so a
/// same-length rewrite in place within one of its ticks keeps every part of the identity. A
/// cache therefore trusts an identity only once it is settled ([`FileKey::settled_before`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileKey {
    len: u64,
    modified: i128,
    changed: i128,
    inode: u64,
    device: u64,
}

impl FileKey {
    /// The identity of the regular file at `path`; `None` when there is none, or it cannot be had.
    pub fn of(path: &Path) -> Option<FileKey> {
        FileKey::of_meta(&std::fs::metadata(path).ok()?)
    }

    /// The identity of the regular file `meta` describes (an open file's, say).
    pub fn of_meta(meta: &std::fs::Metadata) -> Option<FileKey> {
        let nanos = |secs: i64, nsecs: i64| i128::from(secs) * 1_000_000_000 + i128::from(nsecs);
        meta.is_file().then(|| FileKey {
            len: meta.len(),
            modified: nanos(meta.mtime(), meta.mtime_nsec()),
            changed: nanos(meta.ctime(), meta.ctime_nsec()),
            inode: meta.ino(),
            device: meta.dev(),
        })
    }

    /// Its length.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// When it was last written, as a cache file's own identity says when the cache was written.
    pub fn written(&self) -> i128 {
        self.modified
    }

    /// Whether this identity was already settled at `moment`: both its times are older, so that
    /// every later write changes it. A cache may trust an identity only when it was settled
    /// before the cache that holds it was written; one of the cache's own tick could have been
    /// rewritten in place since, its length kept (git calls such a file racily clean).
    pub fn settled_before(&self, moment: i128) -> bool {
        self.modified < moment && self.changed < moment
    }

    /// Whether `other` is the same file: the same inode on the same device, whatever was written.
    pub fn same_file(&self, other: &FileKey) -> bool {
        self.inode == other.inode && self.device == other.device
    }

    /// This identity with the length `len`: a file read up to there.
    pub fn up_to(self, len: u64) -> FileKey {
        FileKey { len, ..self }
    }

    /// Its parts, as a projection keeps them: length, modification and change times, inode,
    /// device.
    pub fn parts(&self) -> (u64, i128, i128, u64, u64) {
        (
            self.len,
            self.modified,
            self.changed,
            self.inode,
            self.device,
        )
    }

    /// The identity of these parts ([`FileKey::parts`]).
    pub fn from_parts(
        (len, modified, changed, inode, device): (u64, i128, i128, u64, u64),
    ) -> FileKey {
        FileKey {
            len,
            modified,
            changed,
            inode,
            device,
        }
    }
}

/// What the memo holds of one file: its identity then, and what was learnt of it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Remembered {
    key: FileKey,
    /// How many lines it held.
    lines: Option<usize>,
    /// The blob `sync.git` made of it.
    blob: Option<String>,
}

/// The store's memo of its journal files, by their paths in the store (`events/<machine>/...`).
#[derive(Clone, Debug, Default)]
pub struct Memo {
    files: BTreeMap<String, Remembered>,
    /// Whether it differs from what was read.
    changed: bool,
    /// Whether it holds only some of the files the memo on disk does ([`Memo::load_only`]): it is
    /// then never written back.
    partial: bool,
}

impl Memo {
    /// The memo of `ws` as it is on disk, of the files that were settled when it was written;
    /// none when it is missing, cannot be read whole, or its own identity cannot be had. Its
    /// bytes and its own identity come from one open file, since a writer under the lock can
    /// rename a newer memo over this one between two looks at the path: the moment the one we
    /// read was written is the only one its files may be settled before.
    pub fn load(ws: &Workspace) -> Memo {
        Memo::load_of(ws, &|_| true, MemoRead::Whole)
    }

    /// [`Memo::load`], keeping only the files `wanted` names by their paths in the store: what one
    /// that asks for a few files' lines takes, the others only checked as a memo is read whole
    /// (roll-lqjj.30: an attempt file's entry is otherwise paid for by every command). Never
    /// written back.
    pub fn load_only(ws: &Workspace, wanted: &dyn Fn(&str) -> bool) -> Memo {
        Memo::load_of(ws, wanted, MemoRead::Partial)
    }

    fn load_of(ws: &Workspace, wanted: &dyn Fn(&str) -> bool, partial: MemoRead) -> Memo {
        let partial = matches!(partial, MemoRead::Partial);
        let (data, key) = read_keyed(&memo_path(ws)).unwrap_or_default();
        let mut files = parse(&data, wanted).unwrap_or_default();
        settled(&mut files, key.map(|memo| memo.written()));
        Memo {
            files,
            changed: false,
            partial,
        }
    }

    /// The blob remembered for `path`, if the file's identity is still `key`.
    pub fn blob(&self, path: &str, key: &FileKey) -> Option<&str> {
        self.files
            .get(path)
            .filter(|file| file.key == *key)
            .and_then(|file| file.blob.as_deref())
    }

    /// How many lines the file at `file` (`path` in the store) holds: as remembered while its
    /// identity is unchanged, else counted now and remembered. A file that cannot be read is the IO
    /// error naming it.
    pub fn lines(&mut self, path: &str, file: &Path) -> Result<usize, Error> {
        let key = FileKey::of(file);
        if let Some(key) = &key
            && let Some(lines) = self
                .files
                .get(path)
                .filter(|known| known.key == *key)
                .and_then(|known| known.lines)
        {
            return Ok(lines);
        }
        let lines = newlines(&read(file)?);
        if let Some(key) = key {
            let blob = self.blob(path, &key).map(str::to_owned);
            self.set(path, key, Some(lines), blob);
        }
        Ok(lines)
    }

    /// Remembers that the file at `path`, whose identity was `key` when it was hashed, is `blob`.
    pub fn hashed(&mut self, path: &str, key: FileKey, blob: &str) {
        let lines = self
            .files
            .get(path)
            .filter(|known| known.key == key)
            .and_then(|known| known.lines);
        self.set(path, key, lines, Some(blob.to_owned()));
    }

    /// Remembers what bead has just written to `file` (`path` in the store): `data`, whose blob is
    /// `blob` when it is known. Nothing is remembered of a file whose identity cannot be had.
    pub fn wrote(&mut self, path: &str, file: &Path, data: &[u8], blob: Option<&str>) {
        if let Some(key) = FileKey::of(file) {
            self.set(path, key, Some(newlines(data)), blob.map(str::to_owned));
        }
    }

    /// Forgets every blob, keeping what else is remembered: each file is hashed again.
    pub fn forget_blobs(&mut self) {
        for file in self.files.values_mut() {
            self.changed |= file.blob.take().is_some();
        }
    }

    /// Forgets every file but those `keep` names: the ones the store holds now.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        let before = self.files.len();
        self.files.retain(|path, _| keep(path));
        self.changed |= self.files.len() != before;
    }

    /// Writes the memo to `ws`'s `sync.git` if it changed since it was read or last written. The
    /// store lock is held. A memo that cannot be written stays as it was: the next command reads
    /// or hashes again what it would have remembered.
    pub fn save(&mut self, ws: &Workspace) {
        if !self.changed || self.partial || !ws.dir.join("sync.git").is_dir() {
            return;
        }
        let failpoints = &ws.settings.failpoints;
        if write_atomic(
            &memo_path(ws),
            &self.render(),
            FileMode::Private,
            failpoints,
        )
        .is_ok()
        {
            self.changed = false;
        }
    }

    fn set(&mut self, path: &str, key: FileKey, lines: Option<usize>, blob: Option<String>) {
        let file = Remembered { key, lines, blob };
        if self.files.get(path) != Some(&file) {
            self.files.insert(path.to_owned(), file);
            self.changed = true;
        }
    }

    /// The memo as it is written.
    fn render(&self) -> Vec<u8> {
        let mut out = HEADER.to_vec();
        for (path, file) in &self.files {
            let FileKey {
                len,
                modified,
                changed,
                inode,
                device,
            } = file.key;
            let lines = file.lines.map_or_else(|| "-".to_owned(), |n| n.to_string());
            let blob = file.blob.as_deref().unwrap_or("-");
            out.extend_from_slice(path.as_bytes());
            out.push(0);
            out.extend_from_slice(
                format!("{len} {modified} {changed} {inode} {device} {lines} {blob}\n").as_bytes(),
            );
        }
        out
    }
}

/// Keeps of `files` what a memo written at `written` may say: those settled by then
/// ([`FileKey::settled_before`]). Without a time for the memo itself, nothing is remembered.
fn settled(files: &mut BTreeMap<String, Remembered>, written: Option<i128>) {
    match written {
        Some(written) => files.retain(|_, file| file.key.settled_before(written)),
        None => files.clear(),
    }
}

/// The files a memo holds that `wanted` names, or `None` when any part of it is not as
/// [`Memo::render`] writes it: each file once, in the order of their paths. Every line is checked
/// whole, as one is written, whichever are asked for (what it says of one file never depends on
/// which), but only the lines asked for are made into values: the others are checked for their
/// shape alone ([`fields_are_written`]), which costs under half of making them (roll-lqjj.47: the
/// 92,000 attempt files of an ownership-heavy tracker were 47 ms a command, for a count that
/// reads the segments' lines alone).
fn parse(data: &[u8], wanted: &dyn Fn(&str) -> bool) -> Option<BTreeMap<String, Remembered>> {
    // every byte is a part of a path or of the fields behind it, so that text is checked as a whole
    let mut rest = std::str::from_utf8(data.strip_prefix(HEADER)?).ok()?;
    let mut files = BTreeMap::new();
    let mut last: Option<&str> = None;
    while !rest.is_empty() {
        let nul = rest.find('\0')?;
        let path = rest.get(..nul)?;
        if path.is_empty() || last.is_some_and(|last| path <= last) {
            return None;
        }
        last = Some(path);
        let after = rest.get(nul + 1..)?;
        let end = after.find('\n')?;
        let fields = after.get(..end)?;
        if wanted(path) {
            let (key, lines, blob) = fields_of(fields)?;
            files.insert(
                path.to_owned(),
                Remembered {
                    key,
                    lines,
                    blob: blob.map(str::to_owned),
                },
            );
        } else if !fields_are_written(fields) {
            return None;
        }
        rest = after.get(end + 1..)?;
    }
    Some(files)
}

/// What the fields of one file's line say, or `None` when they are not as [`Memo::render`] writes
/// them: the identity, the lines (`-` when not known) and the blob (`-` when not known).
fn fields_of(fields: &str) -> Option<(FileKey, Option<usize>, Option<&str>)> {
    let mut fields = fields.split(' ');
    let [len, modified, changed, inode, device, lines, blob] = [(); 7].map(|()| fields.next());
    if fields.next().is_some() {
        return None;
    }
    let key = FileKey {
        len: number(len?)?,
        modified: number(modified?)?,
        changed: number(changed?)?,
        inode: number(inode?)?,
        device: number(device?)?,
    };
    let lines = match lines? {
        "-" => None,
        lines => Some(number(lines)?),
    };
    let blob = match blob? {
        "-" => None,
        blob if is_object_name(blob) => Some(blob),
        _ => return None,
    };
    Some((key, lines, blob))
}

/// Whether `fields` are as [`fields_of`] needs them, which it answers without making the numbers
/// of the fields: a run of digits that is shorter than every number of its type can be long is a
/// number of it ([`number_is_written`]).
fn fields_are_written(fields: &str) -> bool {
    let mut fields = fields.split(' ');
    let [len, modified, changed, inode, device, lines, blob] = [(); 7].map(|()| fields.next());
    let (
        Some(len),
        Some(modified),
        Some(changed),
        Some(inode),
        Some(device),
        Some(lines),
        Some(blob),
    ) = (len, modified, changed, inode, device, lines, blob)
    else {
        return false;
    };
    fields.next().is_none()
        && number_is_written::<u64>(len, 19)
        && number_is_written::<i128>(modified, 38)
        && number_is_written::<i128>(changed, 38)
        && number_is_written::<u64>(inode, 19)
        && number_is_written::<u64>(device, 19)
        && (lines == "-" || number_is_written::<usize>(lines, 9))
        && (blob == "-" || is_object_name(blob))
}

/// Whether `text` is a number of type `N` as [`Memo::render`] writes it ([`number`]); `safe` is
/// how many digits every number of `N` fits in, so that none longer than that is converted to
/// tell.
fn number_is_written<N: std::str::FromStr>(text: &str, safe: usize) -> bool {
    let plain = !text.is_empty() && text.len() <= safe && text.bytes().all(|b| b.is_ascii_digit());
    plain || number::<N>(text).is_some()
}

/// A number as [`Memo::render`] writes it: ASCII digits, a minus sign first for a negative time.
fn number<N: std::str::FromStr>(text: &str) -> Option<N> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// A SHA-1 or SHA-256 object name, as git prints it.
fn is_object_name(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The number of newline bytes in `data`.
fn newlines(data: &[u8]) -> usize {
    data.iter().filter(|b| **b == b'\n').count()
}

#[cfg(test)]
mod tests;
