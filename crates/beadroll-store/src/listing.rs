//! Accepted attempt names reused within a command's locked phase. Directory metadata vouches
//! only for the names: callers still stat each file before trusting its contents or projection.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::Error;
use crate::fs::{dir_entries, has_suffix};

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

/// The accepted attempt directories observed in one workspace. Missing or unreadable metadata
/// never authorizes reuse. The combined listing shares its paths across callers until a
/// directory changes, and each directory is read again only when its own identity changes.
#[derive(Debug, Default)]
pub(crate) struct Claims {
    directories: HashMap<PathBuf, (DirectoryKey, Arc<[PathBuf]>)>,
    sources: Vec<Arc<[PathBuf]>>,
    files: Arc<[PathBuf]>,
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

    /// How many directories' names are kept: what a write into one must forget.
    #[cfg(test)]
    pub(crate) fn listed(&self) -> usize {
        self.directories.len()
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
