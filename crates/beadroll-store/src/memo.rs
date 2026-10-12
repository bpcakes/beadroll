//! What the store remembers of its journal files between commands (roll-lqjj.1): for each segment
//! and attempt file, by its path in the store, the identity the file had on disk when bead last
//! read or hashed it, how many lines it held then, and the blob `sync.git` made of it. A full
//! segment is never appended to again and an attempt file never changes, so a file whose identity
//! is the remembered one need not be read or hashed again; any other file is.
//!
//! The memo is a cache kept in `sync.git`, and goes with it when `sync.git` is set aside or
//! deleted. A memo that is missing, or cannot be read whole, counts as none; one that cannot be
//! written is left as it was, since nothing depends on it. Only the holder of the store lock
//! writes it, whole, under a temporary name. A remembered blob only says what `sync.git` made of a
//! file once: whoever uses one makes sure the object is still there.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::fs::{read, write_atomic};
use crate::workspace::Workspace;

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
/// into place) the inode, and any write the times.
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
}

impl Memo {
    /// The memo of `ws` as it is on disk; none when it is missing or cannot be read whole.
    pub fn load(ws: &Workspace) -> Memo {
        let files = std::fs::read(memo_path(ws))
            .ok()
            .and_then(|data| parse(&data))
            .unwrap_or_default();
        Memo {
            files,
            changed: false,
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
        if !self.changed || !ws.dir.join("sync.git").is_dir() {
            return;
        }
        let failpoints = &ws.settings.failpoints;
        if write_atomic(&memo_path(ws), &self.render(), true, failpoints).is_ok() {
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

/// The files a memo holds, or `None` when any part of it is not as [`Memo::render`] writes it.
fn parse(data: &[u8]) -> Option<BTreeMap<String, Remembered>> {
    let mut rest = data.strip_prefix(HEADER)?;
    let mut files = BTreeMap::new();
    while !rest.is_empty() {
        let nul = rest.iter().position(|b| *b == 0)?;
        let path = std::str::from_utf8(rest.get(..nul)?).ok()?;
        let after = rest.get(nul + 1..)?;
        let end = after.iter().position(|b| *b == b'\n')?;
        let fields = std::str::from_utf8(after.get(..end)?).ok()?;
        let fields: Vec<&str> = fields.split(' ').collect();
        let [len, modified, changed, inode, device, lines, blob] = fields.as_slice() else {
            return None;
        };
        let key = FileKey {
            len: number(len)?,
            modified: number(modified)?,
            changed: number(changed)?,
            inode: number(inode)?,
            device: number(device)?,
        };
        let lines = match *lines {
            "-" => None,
            lines => Some(number(lines)?),
        };
        let blob = match *blob {
            "-" => None,
            blob if is_object_name(blob) => Some(blob.to_owned()),
            _ => return None,
        };
        let file = Remembered { key, lines, blob };
        if path.is_empty() || files.insert(path.to_owned(), file).is_some() {
            return None;
        }
        rest = after.get(end + 1..)?;
    }
    Some(files)
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

/// How many lines `data` holds (Python's `data.count(b"\n")`).
fn newlines(data: &[u8]) -> usize {
    data.iter().filter(|b| **b == b'\n').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(len: u64) -> FileKey {
        FileKey {
            len,
            modified: 1_791_558_245_123_456_789,
            changed: -5,
            inode: 42,
            device: u64::MAX,
        }
    }

    fn remembered(len: u64, lines: Option<usize>, blob: Option<&str>) -> Remembered {
        Remembered {
            key: key(len),
            lines,
            blob: blob.map(str::to_owned),
        }
    }

    #[test]
    fn a_memo_reads_back_as_it_was_written() {
        let mut memo = Memo::default();
        let sha1 = "0123456789abcdef0123456789abcdef01234567";
        let sha256 = "ab".repeat(32);
        memo.set("events/m/000001.jsonl", key(10), Some(3), Some(sha1.into()));
        memo.set("events/m/claims/x y.json", key(0), None, Some(sha256));
        memo.set("events/odd\nname/000002.jsonl", key(7), Some(1), None);
        let written = memo.render();
        assert!(written.starts_with(HEADER));
        assert_eq!(parse(&written), Some(memo.files.clone()));
        assert_eq!(parse(HEADER), Some(BTreeMap::new()));
    }

    #[test]
    fn a_memo_that_is_not_whole_is_none() {
        let mut memo = Memo::default();
        let sha = "0123456789abcdef0123456789abcdef01234567";
        memo.set("events/m/000001.jsonl", key(10), Some(3), Some(sha.into()));
        let written = memo.render();
        for cut in 0..written.len() {
            // a record cut short anywhere is no memo, never part of one
            if cut != HEADER.len() {
                assert_eq!(parse(&written[..cut]), None, "{cut}");
            }
        }
        let text = String::from_utf8(written.clone()).unwrap();
        for wrong in [
            text.replace("bead journal memo 1", "bead journal memo 2"),
            text.replace(" 3 ", " +3 "),
            text.replace(" 3 ", " three "),
            text.replace(sha, &sha.to_uppercase()),
            text.replace(sha, &sha[..39]),
            text.replace(" 42 ", " 42  "),
            text.replace("events/m/000001.jsonl", ""),
            format!("{text}{}", &text[HEADER.len()..]),
        ] {
            assert_eq!(parse(wrong.as_bytes()), None, "{wrong:?}");
        }
        let mut invalid = written;
        invalid.insert(HEADER.len(), 0xff);
        assert_eq!(parse(&invalid), None);
    }

    #[test]
    fn a_blob_is_remembered_only_for_the_identity_it_was_hashed_at() {
        let mut memo = Memo::default();
        let sha = "0123456789abcdef0123456789abcdef01234567";
        memo.files
            .insert("p".into(), remembered(10, Some(2), Some(sha)));
        assert_eq!(memo.blob("p", &key(10)), Some(sha));
        assert_eq!(memo.blob("p", &key(11)), None);
        assert_eq!(memo.blob("q", &key(10)), None);
        // hashed again at another identity: its lines are not known for that one
        memo.hashed("p", key(11), sha);
        assert_eq!(memo.files["p"], remembered(11, None, Some(sha)));
        memo.hashed("p", key(11), sha);
        memo.changed = false;
        memo.hashed("p", key(11), sha);
        assert!(!memo.changed, "nothing new was learnt");
        memo.files
            .insert("q".into(), remembered(3, Some(1), Some(sha)));
        memo.changed = false;
        memo.forget_blobs();
        assert!(memo.changed, "a blob was forgotten");
        assert_eq!(
            memo.files["q"],
            remembered(3, Some(1), None),
            "its lines are kept"
        );
        assert_eq!(memo.blob("q", &key(3)), None);
        memo.retain(|path| path != "p" && path != "q");
        assert!(memo.changed && memo.files.is_empty());
    }
}
