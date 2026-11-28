//! The operating-system lock and the cache phase it protects have one lifetime.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt as _;

use beadroll_git::IoError;

use super::{LockState, Workspace};
use crate::{Error, fs::FileMode, layout, private_dir, private_dirs};

/// An exclusive store lock. Dropping it invalidates the phase's cached identities before
/// releasing the file lock, including when a caller unwinds.
#[derive(Debug)]
#[must_use = "dropping the guard releases the store lock"]
pub struct WorkspaceLock<'a> {
    workspace: &'a Workspace,
    file: File,
}

impl Workspace {
    /// Locks this store and begins a fresh cache phase. The guard must outlive all work in that
    /// phase. Like the underlying file lock, this is not a reentrant lock.
    /// Bind the returned guard: discarding it immediately releases the lock and warns.
    ///
    /// ```compile_fail
    /// #![deny(unused_must_use)]
    /// # fn discarded(ws: &beadroll_store::Workspace) -> Result<(), beadroll_store::Error> {
    /// ws.lock()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn lock(&self) -> Result<WorkspaceLock<'_>, Error> {
        private_dir(&self.settings.home()?)?;
        private_dir(&self.dir)?;
        private_dirs(&self.events_dir)?;
        let path = self.dir.join(layout::LOCK);
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(FileMode::Private.bits())
            .open(&path)
            .map_err(|error| IoError::at(&error, &path))?;
        file.lock().map_err(|error| IoError::at(&error, &path))?;
        self.set_holding(LockState::Locked);
        Ok(WorkspaceLock {
            workspace: self,
            file,
        })
    }
}

impl Drop for WorkspaceLock<'_> {
    fn drop(&mut self) {
        self.workspace.set_holding(LockState::Unlocked);
        // Unlocked here, not when the file closes: a lock taken with `flock` lasts until every
        // descriptor of the open file is closed, and a child that another thread is starting holds
        // a copy of each until its exec closes them, so that the store would stay locked for a
        // moment after the guard is gone (roll-x9x5). Should the unlock fail, the close still
        // releases it.
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests;
