//! Store filesystem rules: private permissions regardless of umask, atomic whole-file writes and
//! directory listings that distinguish absence from unreadability.

use std::ffi::OsStr;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use beadroll_core::json::{self, Surrogates, Value};
use beadroll_git::IoError;

use crate::error::Error;
use crate::failpoint::{Failpoints, Fired};
use crate::memo::FileKey;

/// `path` and any missing parents made as 0700 (less the umask, as `mkdir` gives it); a race loser
/// is fine, a file in the way is an error.
pub fn private_dirs(path: &Path) -> Result<(), Error> {
    let mut missing = Vec::new();
    let mut at = path;
    while !at.is_dir() {
        let Some(parent) = at.parent().filter(|p| *p != at) else {
            break;
        };
        missing.push(at);
        at = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
    }
    for folder in missing.into_iter().rev() {
        if let Err(error) = DirBuilder::new().mode(0o700).create(folder)
            && !(error.kind() == io::ErrorKind::AlreadyExists && folder.is_dir())
        {
            return Err(IoError::at(&error, folder).into());
        }
    }
    Ok(())
}

/// A directory only its owner can enter: the store and everything beneath it.
pub fn private_dir(path: &Path) -> Result<(), Error> {
    private_dirs(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| IoError::at(&e, path).into())
}

/// `data` written whole to a temporary file beside `path` (`.NAME.PID.tmp`), fsynced, then renamed
/// into place. Store files are 0600; a file written into the caller's checkout (`private` false)
/// follows the caller's umask like any other file there. A write that fails leaves nothing behind.
/// The failpoints `atomic-write:<part>` (before the rename) and `atomic-written:<part>` (after
/// it) fire for a target whose path holds `<part>`.
pub fn write_atomic(
    path: &Path,
    data: &[u8],
    private: bool,
    failpoints: &Failpoints,
) -> Result<(), Error> {
    write_atomic_parts(path, &[data], private, failpoints)
}

/// [`write_atomic`] of `parts`, one after the other, as one file: a large file made of a few parts
/// is written without first being joined in memory.
pub(crate) fn write_atomic_parts(
    path: &Path,
    parts: &[&[u8]],
    private: bool,
    failpoints: &Failpoints,
) -> Result<(), Error> {
    written_whole(path, parts, private, true, failpoints).map(drop)
}

/// [`write_atomic`] of a store file, with the identity of the file written, taken from its own
/// descriptor once it is in place (an identity not yet settled, which `docs/REVIEWING.md` §2
/// weighs for `meta.json`).
pub(crate) fn write_atomic_keyed(
    path: &Path,
    data: &[u8],
    failpoints: &Failpoints,
) -> Result<Option<FileKey>, Error> {
    let file = written_whole(path, &[data], true, true, failpoints)?;
    Ok(file.metadata().ok().as_ref().and_then(FileKey::of_meta))
}

/// [`write_atomic_parts`] of a store cache, without the fsync: the projection and its text file,
/// whose every part a reader checks against a sum kept of it and which bead makes again from the
/// journal whenever it is missing, cut, damaged or out of step with the other. Renamed into place
/// whole, so that no reader sees it half-written; a crash may leave it lost, cut, zeroed or as it
/// was before, and each costs a replay (`docs/REVIEWING.md` §2), where flushing it cost the
/// command after every write as much again as the write (X05). The failpoints fire as for
/// [`write_atomic`].
pub(crate) fn write_cache_parts(
    path: &Path,
    parts: &[&[u8]],
    failpoints: &Failpoints,
) -> Result<(), Error> {
    written_whole(path, parts, true, false, failpoints).map(drop)
}

/// `parts` written to a temporary file beside `path`, fsynced when `durable`, and renamed into
/// place, the file written still open: [`write_atomic_parts`], [`write_atomic_keyed`] and
/// [`write_cache_parts`].
fn written_whole(
    path: &Path,
    parts: &[&[u8]],
    private: bool,
    durable: bool,
    failpoints: &Failpoints,
) -> Result<File, Error> {
    let parent = path.parent().unwrap_or(Path::new("."));
    if private {
        private_dirs(parent)?;
    } else {
        std::fs::create_dir_all(parent).map_err(|e| Error::Io(IoError::at(&e, parent)))?;
    }
    let tmp = temporary(path);
    let mode = if private { 0o600 } else { 0o666 };
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .map_err(|e| Error::Io(IoError::at(&e, &tmp)))?;
    let written = (parts.iter())
        .try_for_each(|part| file.write_all(part))
        .map_err(|e| IoError::bare(&e))
        .and_then(|()| if durable { fsync(&file) } else { Ok(()) })
        .and_then(|()| match failpoints.hit_path("atomic-write", path) {
            // the write fails as a full volume fails it
            Some(Fired::Errno(errno)) => Err(IoError::bare(&io::Error::from_raw_os_error(errno))),
            _ => Ok(()),
        })
        .and_then(|()| std::fs::rename(&tmp, path).map_err(|e| IoError::at(&e, path)));
    if let Err(error) = written {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Io(error));
    }
    failpoints.hit_path("atomic-written", path);
    Ok(file)
}

/// `.NAME.PID.tmp` beside `path`, its name's bytes kept as they are.
fn temporary(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default();
    let mut raw = b".".to_vec();
    raw.extend_from_slice(name.as_bytes());
    raw.extend_from_slice(format!(".{}.tmp", std::process::id()).as_bytes());
    path.with_file_name(OsStr::from_bytes(&raw))
}

/// Uses fsync(2), never F_FULLFSYNC (`docs/REVIEWING.md` §2).
pub fn fsync(file: &File) -> Result<(), IoError> {
    rustix::fs::fsync(file).map_err(|e| IoError::bare(&io::Error::from(e)))
}

/// A directory's entries, sorted by name. Absent is empty; one that cannot be read, or is not a
/// directory, is an IO error, never an empty journal.
pub fn dir_entries(folder: &Path) -> Result<Vec<PathBuf>, Error> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(IoError::at(&error, folder).into()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::Io(IoError::at(&e, folder)))?;
        paths.push(entry.path());
    }
    // by name: every path is `folder` and a separator followed by one, so their bytes sort as the
    // names do, without taking each path apart at every comparison (roll-lqjj.30)
    paths.sort_unstable_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    Ok(paths)
}

/// Sorted entries of folder whose names end with ending, including hidden names and directories.
/// Return empty if folder is not a directory or cannot be read.
pub fn glob_ending(folder: &Path, ending: &str) -> Vec<PathBuf> {
    if !folder.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.as_bytes().ends_with(ending.as_bytes()))
        })
        .collect();
    // by name, as `dir_entries` sorts
    paths.sort_unstable_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    paths
}

/// Whether the path suffix equals suffix, taking the last dot unless it begins the filename.
pub fn has_suffix(path: &Path, suffix: &str) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let name = name.as_bytes();
    name.len() > suffix.len() && name.ends_with(suffix.as_bytes())
}

/// The bytes of `path`, or the IO error naming it.
pub fn read(path: &Path) -> Result<Vec<u8>, IoError> {
    std::fs::read(path).map_err(|e| IoError::at(&e, path))
}

/// The bytes of `path` up to its length when it was opened, and the identity of what was read:
/// the open file's, up to those bytes, so that a file replaced or appended to while it was read
/// is never taken for what was read (roll-lqjj.2).
pub(crate) fn read_keyed(path: &Path) -> Result<(Vec<u8>, Option<FileKey>), IoError> {
    let file = File::open(path).map_err(|e| IoError::at(&e, path))?;
    let meta = file.metadata().map_err(|e| IoError::at(&e, path))?;
    let mut data = Vec::new();
    file.take(meta.len())
        .read_to_end(&mut data)
        .map_err(|e| IoError::at(&e, path))?;
    let key = FileKey::of_meta(&meta).map(|key| key.up_to(data.len() as u64));
    Ok((data, key))
}

/// A 128-bit sum of `bytes`, the projection's ([`beadroll_core::projection::sum`]): it finds damage
/// and change, not forgery; the store is private.
pub(crate) fn content_sum(bytes: &[u8]) -> [u8; 16] {
    beadroll_core::projection::sum(bytes)
}

/// A file-read, UTF-8 decode or JSON parse failure, formatted with the established diagnostic for
/// that operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unreadable {
    /// The file is not there.
    Missing,
    /// The diagnostic for an unreadable file or invalid JSON.
    Other(String),
}

/// Reads a JSON file as strict UTF-8, rejecting raw encoded surrogates, then parses its JSON.
pub fn read_json_text(path: &Path) -> Result<Value, Unreadable> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(Unreadable::Missing),
        Err(error) => return Err(Unreadable::Other(os_error_str(&error, path))),
    };
    json_text(&data).map_err(Unreadable::Other)
}

/// [`read_json_text`], with the identity of the file read, from the descriptor it was read
/// through: a file renamed over it meanwhile is never taken for the one read.
pub(crate) fn read_json_keyed(path: &Path) -> (Result<Value, Unreadable>, Option<FileKey>) {
    let read = File::open(path).and_then(|mut file| {
        let meta = file.metadata()?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        let key = FileKey::of_meta(&meta).map(|key| key.up_to(data.len() as u64));
        Ok((data, key))
    });
    match read {
        Ok((data, key)) => (json_text(&data).map_err(Unreadable::Other), key),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (Err(Unreadable::Missing), None),
        Err(error) => (Err(Unreadable::Other(os_error_str(&error, path))), None),
    }
}

/// Reads JSON for a whole-file rewrite, preserving lone surrogates with [`Surrogates`]. Allocate
/// a distinct stand-in for each code unit in the text or `units`, avoiding characters in `avoid`.
/// Malformed JSON reports a diagnostic against the original text.
pub fn read_json_keeping_surrogates(
    path: &Path,
    units: &[u16],
    avoid: &str,
) -> Result<(Value, Surrogates), Unreadable> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(Unreadable::Missing),
        Err(error) => return Err(Unreadable::Other(os_error_str(&error, path))),
    };
    let parsed = json_text(&data).map_err(Unreadable::Other)?;
    let text = std::str::from_utf8(&data).unwrap_or_default();
    // There are more required stand-ins than available private-use characters; bead cannot
    // produce such a file.
    let Some((stood, surrogates)) = Surrogates::stand_in(text, units, avoid) else {
        return Err(Unreadable::Other(
            "it holds more lone surrogates and private-use characters than bead can keep apart"
                .to_owned(),
        ));
    };
    if stood == text {
        return Ok((parsed, surrogates));
    }
    let value = json::parse(stood.as_bytes())
        .map(|parsed| parsed.value)
        .map_err(|error| Unreadable::Other(error.to_string()))?;
    Ok((value, surrogates))
}

/// Decodes strict UTF-8 and parses JSON, returning the value or its diagnostic.
pub fn json_text(data: &[u8]) -> Result<Value, String> {
    if let Err(error) = std::str::from_utf8(data) {
        return Err(utf8_error(data, &error));
    }
    json::parse(data)
        .map(|parsed| parsed.value)
        .map_err(|error| error.to_string())
}

/// An operating-system error about `path`, formatted as `[Errno N] strerror: 'path'`.
pub fn os_error_str(error: &io::Error, path: &Path) -> String {
    let strerror = beadroll_git::strerror(error);
    match error.raw_os_error() {
        Some(errno) => format!(
            "[Errno {errno}] {strerror}: {}",
            beadroll_core::text::repr_str(&path.to_string_lossy())
        ),
        None => strerror,
    }
}

/// The established diagnostic for invalid UTF-8 bytes.
pub fn utf8_error(data: &[u8], error: &std::str::Utf8Error) -> String {
    let start = error.valid_up_to();
    let (end, reason) = match error.error_len() {
        None => (data.len(), "unexpected end of data"),
        Some(len) => {
            let first = data.get(start).copied().unwrap_or_default();
            let starts = matches!(first, 0xC2..=0xF4);
            let reason = if starts {
                "invalid continuation byte"
            } else {
                "invalid start byte"
            };
            (start + len, reason)
        }
    };
    if end - start == 1 {
        let byte = data.get(start).copied().unwrap_or_default();
        format!("'utf-8' codec can't decode byte 0x{byte:02x} in position {start}: {reason}")
    } else {
        format!(
            "'utf-8' codec can't decode bytes in position {start}-{}: {reason}",
            end - 1
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listing is in the order of its names' bytes, which whole paths under one folder sort in
    /// too: a name that is a prefix of another, punctuation that sorts before the folder's
    /// separator, upper case, and bytes that are not UTF-8 where the file system takes such a name.
    #[test]
    fn a_listing_is_in_the_order_of_its_names() {
        let dir = tempfile::tempdir().unwrap();
        let names: [&[u8]; 9] = [
            b"a",
            b"a.json",
            b"a-b",
            b"a.b.json",
            b"B",
            b"a0",
            b".hidden",
            b"\xff",
            b"a\x01",
        ];
        let mut sorted: Vec<&[u8]> = Vec::new();
        for name in names {
            match std::fs::write(dir.path().join(OsStr::from_bytes(name)), b"") {
                Ok(()) => sorted.push(name),
                // A file system that keeps names as UTF-8 (APFS) refuses one that is not: there
                // the order of such names is nothing a listing can be asked for.
                Err(error)
                    if error.raw_os_error() == Some(rustix::io::Errno::ILSEQ.raw_os_error())
                        && std::str::from_utf8(name).is_err() => {}
                Err(error) => panic!("cannot make {name:?}: {error}"),
            }
        }
        sorted.sort();
        let listed = dir_entries(dir.path()).unwrap();
        let listed: Vec<&[u8]> = (listed.iter())
            .map(|path| path.file_name().unwrap().as_bytes())
            .collect();
        assert_eq!(listed, sorted);
        let globbed = glob_ending(dir.path(), "");
        let globbed: Vec<&[u8]> = (globbed.iter())
            .map(|path| path.file_name().unwrap().as_bytes())
            .collect();
        assert_eq!(globbed, sorted);
    }

    /// What a file read is taken to be is what was read: the file opened, up to the bytes read,
    /// whatever its path names by the time anyone looks again (roll-lqjj.2, P03).
    #[test]
    fn a_file_read_is_known_by_the_file_its_bytes_came_from() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("attempt.json");
        std::fs::write(&path, b"first\n").unwrap();
        let (data, key) = read_keyed(&path).unwrap();
        let key = key.unwrap();
        assert_eq!(data, b"first\n");
        assert_eq!(Some(key), FileKey::of(&path));
        // replaced whole, as a merge replaces an attempt file that differs
        let other = dir.path().join(".attempt.json.tmp");
        std::fs::write(&other, b"second\n").unwrap();
        std::fs::rename(&other, &path).unwrap();
        let now = FileKey::of(&path).unwrap();
        assert!(!now.same_file(&key), "the replacement is another file");
        assert_eq!(read_keyed(&path).unwrap().1, Some(now));
    }

    #[test]
    fn decode_errors_report_bytes_positions_and_reasons() {
        let message = |data: &[u8]| utf8_error(data, &std::str::from_utf8(data).unwrap_err());
        assert_eq!(
            message(b"ab\xff"),
            "'utf-8' codec can't decode byte 0xff in position 2: invalid start byte"
        );
        assert_eq!(
            message(b"\xe4\x41"),
            "'utf-8' codec can't decode byte 0xe4 in position 0: invalid continuation byte"
        );
        assert_eq!(
            message(b"x\xe4\xb8"),
            "'utf-8' codec can't decode bytes in position 1-2: unexpected end of data"
        );
        assert_eq!(
            message(b"\xe4\xb8\x41"),
            "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte"
        );
        assert_eq!(
            message(b"\xed\xa0\x80"),
            "'utf-8' codec can't decode byte 0xed in position 0: invalid continuation byte"
        );
    }

    #[test]
    fn atomic_writes_leave_nothing_behind_and_keep_private_modes() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a/b/file.json");
        write_atomic(&target, b"{}", true, &Failpoints::none()).unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read(&target).unwrap(), b"{}");
        let names: Vec<_> = dir_entries(target.parent().unwrap()).unwrap();
        assert_eq!(names, std::slice::from_ref(&target));
        let blocked = dir.path().join("file-not-dir");
        std::fs::write(&blocked, "").unwrap();
        let error = write_atomic(&blocked.join("x"), b"", true, &Failpoints::none()).unwrap_err();
        assert_eq!(error.code().as_str(), "IO");
    }

    /// A file written from parts holds their bytes one after the other, an empty one among them,
    /// as the write of them joined would (roll-lqjj.9: the text file, its header and its texts).
    #[test]
    fn a_file_written_from_parts_is_their_bytes_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("texts");
        let parts: [&[u8]; 3] = [b"head", b"", b"texts\n"];
        write_atomic_parts(&target, &parts, true, &Failpoints::none()).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"headtexts\n");
        let names: Vec<_> = dir_entries(dir.path()).unwrap();
        assert_eq!(names, std::slice::from_ref(&target));
    }

    /// A cache is written as any store file is but for the fsync: whole, 0600, in place of the
    /// one there or not at all, nothing left behind, and past both failpoints of an atomic write.
    #[test]
    fn a_cache_is_written_whole_or_not_at_all_and_passes_the_failpoints() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a/b/projection-texts");
        let parts: [&[u8]; 3] = [b"head", b"", b"texts\n"];
        write_cache_parts(&target, &parts, &Failpoints::none()).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"headtexts\n");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // the volume full as it is written again: the one there stays, and nothing else
        let spec = "atomic-write:-texts=errno(ENOSPC);atomic-written:-texts@always=sleep(0)";
        let failpoints = Failpoints::from_env(Some(OsStr::new(spec)), false, false).unwrap();
        let error = write_cache_parts(&target, &[b"other"], &failpoints).unwrap_err();
        assert_eq!(error.code().as_str(), "IO");
        assert_eq!(failpoints.hits("atomic-write:-texts"), 1);
        assert_eq!(failpoints.hits("atomic-written:-texts"), 0);
        assert_eq!(std::fs::read(&target).unwrap(), b"headtexts\n");
        write_cache_parts(&target, &[b"other"], &failpoints).unwrap();
        assert_eq!(failpoints.hits("atomic-written:-texts"), 1);
        assert_eq!(std::fs::read(&target).unwrap(), b"other");
        let names: Vec<_> = dir_entries(target.parent().unwrap()).unwrap();
        assert_eq!(names, std::slice::from_ref(&target));
    }

    #[test]
    fn suffixes_and_globs_handle_hidden_names() {
        assert!(has_suffix(Path::new("/x/000001.jsonl"), ".jsonl"));
        assert!(!has_suffix(Path::new("/x/.jsonl"), ".jsonl"));
        assert!(has_suffix(Path::new("/x/.b.json"), ".json"));
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.json", ".b.json", "C.JSON", "d.json.tmp", ".json"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        let found: Vec<String> = glob_ending(dir.path(), ".json")
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(found, [".b.json", ".json", "a.json"]);
    }
}
