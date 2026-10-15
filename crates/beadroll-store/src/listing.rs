//! Accepted attempt names reused within a command's locked phase, and each file's identity taken
//! once in it. Directory metadata vouches only for the names: each file is still stat-ed before its
//! contents or projection are trusted, once a locked phase (roll-lqjj.30), and every time outside
//! the lock.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::Error;
use crate::fs::{dir_entries, has_suffix};
use crate::memo::FileKey;

/// A directory's identity and entry-change times. An in-place file rewrite need not change any
/// of these, so this key must never vouch for the files' bytes.
#[derive(Debug, PartialEq, Eq)]
struct DirectoryKey {
    len: u64,
    device: u64,
    inode: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    mode: u32,
}

impl DirectoryKey {
    fn of(path: &Path) -> Option<DirectoryKey> {
        let meta = std::fs::metadata(path).ok()?;
        meta.is_dir().then(|| DirectoryKey {
            len: meta.len(),
            device: meta.dev(),
            inode: meta.ino(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
            mode: meta.mode(),
        })
    }
}

/// Each file's identity, in the order of a listing; `None` where it could not be had.
pub(crate) type Keys = Arc<[Option<FileKey>]>;

/// The accepted attempt directories observed in one workspace. Missing or unreadable metadata
/// never authorizes reuse. The combined listing shares its paths across callers until a
/// directory changes, and each directory is read again only when its own identity changes.
#[derive(Debug, Default)]
pub(crate) struct Claims {
    directories: HashMap<PathBuf, (DirectoryKey, Arc<[PathBuf]>)>,
    sources: Vec<Arc<[PathBuf]>>,
    files: Arc<[PathBuf]>,
    /// Each file's identity as it was stat-ed in this phase, by the listing it was taken for, kept
    /// only while the store lock is held: no other bead process writes then, and a file bead
    /// writes itself is forgotten.
    keys: Option<(Arc<[PathBuf]>, Keys)>,
    /// The files bead has written since their identities were taken.
    written: Vec<PathBuf>,
}

impl Claims {
    pub(crate) fn files(&mut self, events: &Path) -> Result<Arc<[PathBuf]>, Error> {
        let mut sources = Vec::new();
        let mut present = Vec::new();
        // Machine directories are few, and their type can change without changing events/.
        for folder in dir_entries(events)? {
            if folder.is_dir() {
                let path = folder.join("claims");
                sources.push(self.directory(&path)?);
                present.push(path);
            }
        }
        self.directories.retain(|path, _| present.contains(path));
        if sources.len() != self.sources.len()
            || !sources
                .iter()
                .zip(&self.sources)
                .all(|(a, b)| Arc::ptr_eq(a, b))
        {
            self.files = sources
                .iter()
                .flat_map(|files| files.iter().cloned())
                .collect();
            self.sources = sources;
        }
        Ok(Arc::clone(&self.files))
    }

    /// [`Claims::files`], every directory read again whatever its identity says: a directory that
    /// gains a file within one tick of an earlier listing keeps its identity (X13).
    pub(crate) fn files_afresh(&mut self, events: &Path) -> Result<Arc<[PathBuf]>, Error> {
        self.directories.clear();
        self.files(events)
    }

    /// The identity of each of `files` ([`FileKey::of`]): with `keep` (the store lock held), the
    /// one this phase took, each file stat-ed once; else each stat-ed now. Identities are kept by
    /// the listing they were taken for, and carried over to a new one file by file, since both are
    /// in the order of their paths.
    pub(crate) fn keys(&mut self, files: &Arc<[PathBuf]>, keep: bool) -> Keys {
        if !keep {
            return files.iter().map(|file| FileKey::of(file)).collect();
        }
        if let Some((listed, keys)) = &self.keys
            && Arc::ptr_eq(listed, files)
        {
            return Arc::clone(keys);
        }
        let mut before = (self.keys.iter())
            .flat_map(|(listed, keys)| listed.iter().zip(keys.iter()))
            .peekable();
        let keys: Keys = (files.iter())
            .map(|file| {
                while before.next_if(|(path, _)| *path < file).is_some() {}
                match before.next_if(|(path, _)| *path == file) {
                    Some((_, key)) if !self.written.contains(file) => *key,
                    _ => FileKey::of(file),
                }
            })
            .collect();
        self.keys = Some((Arc::clone(files), Arc::clone(&keys)));
        self.written.clear();
        keys
    }

    /// Forgets the names listed, and the identity of `written`, a file bead has just written: the
    /// other files keep theirs.
    pub(crate) fn forget(&mut self, written: &Path) {
        self.directories.clear();
        self.sources.clear();
        self.files = Arc::from([]);
        if self.keys.is_some() {
            self.written.push(written.to_path_buf());
        }
    }

    /// How many directories' names are kept: what a write into one must forget.
    #[cfg(test)]
    pub(crate) fn listed(&self) -> usize {
        self.directories.len()
    }

    /// How many files' identities are kept, less those bead wrote since.
    #[cfg(test)]
    pub(crate) fn identities(&self) -> usize {
        let kept = self.keys.as_ref().map_or(0, |(listed, _)| listed.len());
        kept - self.written.len()
    }

    /// The names of the directory at `path` as a listing taken before a file was added would hold
    /// them, under the identity the directory has now: what a kernel that stamps a directory from a
    /// coarse clock keeps when a file is added within the tick of its last change (X13).
    #[cfg(test)]
    pub(crate) fn seed_same_tick(&mut self, path: &Path, files: Vec<PathBuf>) {
        if let Some(key) = DirectoryKey::of(path) {
            self.directories
                .insert(path.to_path_buf(), (key, files.into()));
        }
    }

    fn directory(&mut self, path: &Path) -> Result<Arc<[PathBuf]>, Error> {
        let before = DirectoryKey::of(path);
        if let Some((key, files)) = self.directories.get(path)
            && before.as_ref() == Some(key)
        {
            return Ok(Arc::clone(files));
        }
        self.directories.remove(path);
        let files: Arc<[PathBuf]> = dir_entries(path)?
            .into_iter()
            .filter(|file| has_suffix(file, ".json"))
            .collect();
        // A directory changed during enumeration is still the listing readdir gave us, but
        // cannot be remembered as a listing of either identity.
        if let Some(key) = before
            && DirectoryKey::of(path).as_ref() == Some(&key)
        {
            self.directories
                .insert(path.to_path_buf(), (key, Arc::clone(&files)));
        }
        Ok(files)
    }
}

#[cfg(test)]
mod tests;
