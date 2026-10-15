//! The journal files a projection was made from, kept beside it (roll-lqjj.37), so that a caller
//! that goes on from a projection it read earlier in a locked phase can tell whether the journal is
//! still the one it was made from. An ownership round does: it starts its transaction from the
//! projection its merge read (roll-lqjj.29), which is right while nothing has written under
//! `events/` since, and this is what keeps it from being wrong when something does.
//!
//! Only identities are compared ([`FileKey`], as the memo compares files), never the bytes, and the
//! sums that make a stored projection vouch for a file of its own timestamp tick are not used:
//! every write bead makes to a journal file changes its length (an append) or its inode (a file
//! written whole and renamed into place), or the set of files, and with the store lock held nothing
//! else writes. A rewrite in place of the same length within one tick of the clock, or an inode
//! reused by two replacements within one, would go unseen; nothing between a merge's read and its
//! transaction writes a journal file at all, and what a merge writes it writes before it reads.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use beadroll_core::projection::Projection;
use beadroll_core::state::State;

use crate::journal::{Loaded, claim_files_afresh, claim_keys, segment_files};
use crate::listing::{Keys, Stamp};
use crate::memo::FileKey;
use crate::workspace::Workspace;

use super::file::Made;
use super::kept;

/// The store's projection and the journal files it was made from.
#[derive(Debug)]
pub struct Sourced {
    projection: Projection,
    /// `None` when a file's identity could not be had: nothing vouches for the projection then.
    sources: Option<Sources>,
}

impl Sourced {
    pub(super) fn new(projection: Projection, sources: Option<Sources>) -> Sourced {
        Sourced {
            projection,
            sources,
        }
    }

    /// The state of the projection.
    pub fn state(&self) -> &State {
        self.projection.state()
    }

    /// The projection, whatever the journal is now.
    pub fn into_projection(self) -> Projection {
        self.projection
    }

    /// The projection when the journal's files are still the ones it was made from, else `None`:
    /// a segment grown, added, replaced or gone, an attempt file added, replaced or gone. The
    /// caller then reads the store again ([`super::projection`]).
    pub fn into_current(self, ws: &Workspace) -> Option<Projection> {
        let unchanged = self.sources.as_ref().is_some_and(|s| s.unchanged(ws));
        unchanged.then_some(self.projection)
    }
}

/// The journal files a projection was made from, as it was made from them.
#[derive(Debug)]
pub(super) struct Sources {
    /// Each segment's path under the store's `events` and its identity as it was read, by path.
    segments: Vec<(String, FileKey)>,
    /// The accepted attempt files as they were listed, in the order of the listing.
    attempts: Arc<[PathBuf]>,
    /// Each one's identity: the identities the locked phase keeps (roll-lqjj.30).
    keys: Keys,
    /// The identity each claims directory had when they were listed.
    stamp: Stamp,
}

impl Sources {
    /// The sources of a projection made from `segments`, then the attempt files `paths` listed,
    /// each with its identity (`keys`), in claims directories of the identities `stamp` gives;
    /// `None` when one of them has none.
    pub(super) fn of(
        mut segments: Vec<(String, FileKey)>,
        paths: Arc<[PathBuf]>,
        keys: Keys,
        stamp: Option<Stamp>,
    ) -> Option<Sources> {
        if keys.iter().any(Option::is_none) {
            return None;
        }
        segments.sort_by(|a, b| a.0.cmp(&b.0));
        Some(Sources {
            segments,
            attempts: paths,
            keys,
            stamp: stamp?,
        })
    }

    /// The sources of a projection made from the journal `loaded` read in `segments`, each file's
    /// identity as it was read.
    pub(super) fn read(ws: &Workspace, segments: &[PathBuf], loaded: &Loaded) -> Option<Sources> {
        let held = (segments.iter().zip(&loaded.segments))
            .map(|(path, (key, _))| {
                let held = kept(ws, path, *key, true, None)?;
                Some((held.path, held.key))
            })
            .collect::<Option<Vec<_>>>()?;
        let keys: Keys = loaded.attempts.iter().map(|(_, key, _)| *key).collect();
        let (paths, stamp) = &loaded.listing;
        Sources::of(held, Arc::clone(paths), keys, stamp.clone())
    }

    /// The sources of a projection made from `made`'s segments and the attempt files `paths`
    /// listed, with the identity (`keys`) the phase took of each.
    pub(super) fn made(
        made: &Made,
        paths: Arc<[PathBuf]>,
        keys: Keys,
        stamp: Option<Stamp>,
    ) -> Option<Sources> {
        let segments = (made.files.iter())
            .filter(|held| held.segment)
            .map(|held| (held.path.clone(), held.key))
            .collect();
        Sources::of(segments, paths, keys, stamp)
    }

    /// Whether the journal's files are the ones the projection was made from: the same segments
    /// and attempt files, each with the identity it was read with. The segments are listed and
    /// stat-ed (one `readdir` for the store, one for each machine, one `stat` for each segment);
    /// the attempt files are the listing and the identities of the phase, which cost nothing more
    /// while the store lock is held and nothing was written since.
    pub(super) fn unchanged(&self, ws: &Workspace) -> bool {
        self.segments_unchanged(ws) && self.attempts_unchanged(ws)
    }

    fn segments_unchanged(&self, ws: &Workspace) -> bool {
        let Ok(now) = segment_files(ws) else {
            return false;
        };
        now.len() == self.segments.len()
            && now.iter().all(|path| {
                let Some(name) = relative(ws, path) else {
                    return false;
                };
                let at = (self.segments)
                    .binary_search_by(|(held, _)| held.as_str().cmp(name))
                    .ok();
                let was = at.and_then(|at| self.segments.get(at));
                was.is_some_and(|(_, key)| FileKey::of(path) == Some(*key))
            })
    }

    /// The attempt files are the phase's own listing and identities ([`claim_files_afresh`],
    /// [`claim_keys`]), and the claims directories are the same as listed: a file added, removed
    /// or renamed over, by whatever wrote it, changes its directory's identity, which is
    /// compared whether or not the writer told the phase (`Workspace::forget_claims`), as are the
    /// names and the identities the phase keeps of the files. A rewrite in place changes none of
    /// the directory's.
    fn attempts_unchanged(&self, ws: &Workspace) -> bool {
        let Ok((paths, Some(stamp))) = claim_files_afresh(ws) else {
            return false;
        };
        let keys = claim_keys(ws, &paths);
        // the same listing and identities, as the phase gives them again, or the same by content
        (Arc::ptr_eq(&paths, &self.attempts) || *paths == *self.attempts)
            && (Arc::ptr_eq(&keys, &self.keys) || *keys == *self.keys)
            && *stamp == *self.stamp
    }
}

/// `path` under the store's `events`, as a projection names a file.
fn relative<'p>(ws: &Workspace, path: &'p Path) -> Option<&'p str> {
    path.strip_prefix(&ws.events_dir).ok()?.to_str()
}
