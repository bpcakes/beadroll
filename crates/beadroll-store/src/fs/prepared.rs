//! A whole file prepared before its atomic installation. Keeping preparation apart lets a merge
//! flush several independent new files concurrently, then install them in journal order.

use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use beadroll_git::IoError;

use super::{Durability, FileMode, fsync, private_dirs, temporary};
use crate::Error;
use crate::failpoint::{Failpoints, Fired};

/// A complete temporary file awaiting atomic installation. Dropping it removes the temporary
/// file without replacing the target. Preparation and installation must share the writer's
/// exclusion of other writes to the same path, as [`super::write_atomic`] requires.
#[must_use = "install the prepared file, or drop it to discard it"]
pub struct PreparedAtomic {
    target: PathBuf,
    temporary: PathBuf,
    file: Option<File>,
}

/// Writes and fsyncs a private file beside `path`, without replacing `path` yet. The returned
/// value can be moved between threads, and can be installed only once. Every preparation for
/// one target must finish or be dropped before another begins for that target.
pub fn prepare_atomic(path: &Path, data: &[u8]) -> Result<PreparedAtomic, Error> {
    parts(path, &[data], FileMode::Private, Durability::Durable)
}

pub(super) fn parts(
    path: &Path,
    parts: &[&[u8]],
    mode: FileMode,
    durability: Durability,
) -> Result<PreparedAtomic, Error> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let private = matches!(mode, FileMode::Private);
    if private {
        private_dirs(parent)?;
    } else {
        std::fs::create_dir_all(parent).map_err(|e| IoError::at(&e, parent))?;
    }
    let temporary = temporary(path);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(if private { 0o600 } else { 0o666 })
        .open(&temporary)
        .map_err(|e| IoError::at(&e, &temporary))?;
    let prepared = PreparedAtomic {
        target: path.to_owned(),
        temporary,
        file: Some(file),
    };
    let mut file = prepared
        .file
        .as_ref()
        .ok_or_else(|| Error::io_message("missing prepared file"))?;
    for part in parts {
        file.write_all(part).map_err(|e| IoError::bare(&e))?;
    }
    if matches!(durability, Durability::Durable) {
        fsync(file)?;
    }
    Ok(prepared)
}

impl PreparedAtomic {
    /// Installs the complete file, with the same before/after-rename failpoints as `write_atomic`.
    pub fn install(self, failpoints: &Failpoints) -> Result<(), Error> {
        self.install_file(failpoints).map(drop)
    }

    pub(super) fn install_file(mut self, failpoints: &Failpoints) -> Result<File, Error> {
        if let Some(Fired::Errno(errno)) = failpoints.hit_path("atomic-write", &self.target) {
            return Err(IoError::bare(&io::Error::from_raw_os_error(errno)).into());
        }
        std::fs::rename(&self.temporary, &self.target)
            .map_err(|e| IoError::at(&e, &self.target))?;
        failpoints.hit_path("atomic-written", &self.target);
        self.file
            .take()
            .ok_or_else(|| Error::io_message("missing prepared file"))
    }
}

impl Drop for PreparedAtomic {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.temporary);
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;
    use crate::CrashHooks;

    #[test]
    fn preparation_keeps_the_old_target_until_installation_and_drop_discards_it() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("file");
        std::fs::write(&target, b"old").unwrap();
        let prepared = prepare_atomic(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(std::fs::read(&prepared.temporary).unwrap(), b"new");
        let temporary = prepared.temporary.clone();
        drop(prepared);
        assert!(!temporary.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        prepare_atomic(&target, b"new")
            .unwrap()
            .install(&Failpoints::none())
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(
            std::fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn failed_installation_preserves_the_target_and_removes_the_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("file");
        std::fs::write(&target, b"old").unwrap();
        let prepared = prepare_atomic(&target, b"new").unwrap();
        let temporary = prepared.temporary.clone();
        let faults = Failpoints::from_env(
            Some(OsStr::new("atomic-write:file=errno(ENOSPC)")),
            CrashHooks::default(),
        )
        .unwrap();
        assert!(prepared.install(&faults).is_err());
        assert_eq!(faults.hits("atomic-write:file"), 1);
        assert_eq!(std::fs::read(target).unwrap(), b"old");
        assert!(!temporary.exists());
    }
}
