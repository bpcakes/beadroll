//! Bringing the remote's journals in (Python's `Link.merge`).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use beadroll_store::{
    Memo, complete_prefix, extend_segment, pending_files, projection, read, read_attempt,
    repair_tail, replica_id, segment_files, write_atomic,
};

use super::{Link, newlines};
use crate::error::Error;

const OWN_FORK: &str = "this machine's journal differs from the published one: the same machine \
                        identity is being used from two places. Give one of them a new identity.";
const OWN_CHAIN: &str = "this machine's journal no longer forms one chain: the same machine \
                         identity has written from two places. Give one of them a new identity.";

/// `events/<machine>/claims/<id>.json`, the id of `[0-9a-f-]+`.
fn attempt_shaped(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let [events, machine, claims, file] = parts.as_slice() else {
        return false;
    };
    let Some(id) = file.strip_suffix(".json") else {
        return false;
    };
    *events == "events"
        && !machine.is_empty()
        && *claims == "claims"
        && !id.is_empty()
        && id
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

/// `events/<machine>/<digits>.jsonl`.
fn segment_shaped(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let [events, machine, file] = parts.as_slice() else {
        return false;
    };
    let Some(number) = file.strip_suffix(".jsonl") else {
        return false;
    };
    *events == "events"
        && !machine.is_empty()
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
}

/// `NAME.fork-<ns>` beside `target`.
fn fork_path(link: &Link<'_>, target: &Path) -> PathBuf {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    target.with_file_name(format!("{name}.fork-{}", link.ws.settings.clock.now_ns()))
}

/// A file's stem: its name without the last suffix (pathlib's `stem`).
fn stem(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match name.rfind('.') {
        Some(at) if at > 0 => name[..at].to_owned(),
        _ => name.into_owned(),
    }
}

impl Link<'_> {
    /// Brings the remote's journals in, at `tip`. Each segment has one writer, so one side is
    /// always a prefix of the other. A foreign segment that is not a prefix either way was damaged
    /// here (no machine writes another's journal): the published copy replaces it and ours is kept
    /// beside it. Our own segment in that state, or our own journal no longer forming one chain,
    /// means this machine's identity is in use somewhere else. Attempt files are immutable: a remote
    /// one we lack is copied, and one of our own pending attempts found on the remote is thereby
    /// confirmed. Returns how many events came in. Called with the store lock held.
    ///
    /// A file whose identity is the one the memo remembers is not hashed, and not read when its
    /// remembered blob is the published one. The published tree's blobs are kept for the publish
    /// that follows ([`Link::local_blobs`]).
    pub fn merge(&mut self, tip: Option<&str>) -> Result<i64, Error> {
        self.remote_counts.clear();
        self.forks.clear();
        self.confirmed.clear();
        self.confirmed_events.clear();
        // the memo as it is now: another bead process may have written it since this link last did
        self.memo = Some(Memo::load(self.ws));
        self.listed.clear();
        let mut pulled: i64 = 0;
        for path in segment_files(self.ws)? {
            repair_tail(self.ws, &path)?;
        }
        let own = replica_id(&self.ws.settings, false)?;
        let pending: HashMap<String, PathBuf> = pending_files(self.ws)
            .into_iter()
            .map(|path| (stem(&path), path))
            .collect();
        if let Some(tip) = tip {
            let local = self.local_blobs(false)?;
            for (path, sha) in self.listing(tip)? {
                let Ok(path) = String::from_utf8(path) else {
                    continue; // a name no bead writes
                };
                self.listed.insert(path.clone(), sha.clone());
                if attempt_shaped(&path) {
                    if self.merge_attempt(&path, &sha, &local, &pending)? {
                        pulled += 1;
                    }
                    continue;
                }
                if segment_shaped(&path) {
                    pulled += self.merge_segment(&path, &sha, &local, own.as_deref())?;
                }
            }
        }
        self.save_memo();
        // the store's projection, the records merged applied to it, or the journal replayed
        let merged = projection(self.ws)?;
        if let Some(own) = &own
            && merged.state().chain_breaks().contains_key(own)
        {
            return Err(Error::JournalFork(OWN_CHAIN.to_owned()));
        }
        Ok(pulled)
    }

    /// One remote attempt file: confirms a pending attempt of ours, copies a new one, or replaces
    /// a local copy that differs (kept beside it). Whether it was new here.
    fn merge_attempt(
        &mut self,
        path: &str,
        sha: &str,
        local: &BTreeMap<String, String>,
        pending: &HashMap<String, PathBuf>,
    ) -> Result<bool, Error> {
        self.remote_counts.insert(path.to_owned(), 1);
        let attempt_id = stem(Path::new(path));
        let target = self.ws.dir.join(path);
        let ours_pending = pending.get(&attempt_id);
        if local.get(path).map(String::as_str) == Some(sha) && ours_pending.is_none() {
            return Ok(false);
        }
        let theirs = self.blob(sha)?;
        if let Some(pending_path) = ours_pending {
            // the remote has our attempt: it counts, whole
            if read(pending_path)? != theirs {
                if target.exists() && read(&target)? == theirs {
                    // a leftover twin of a confirmed attempt; its receipt stays: it was accepted
                    crate::tx::remove(pending_path)?;
                    return Ok(false);
                }
                return Err(Error::AttemptFork(format!(
                    "{path} on the remote is not the attempt this machine has pending under that \
                     id: the machine identity is in use somewhere else"
                )));
            }
            if local.get(path).map(String::as_str) != Some(sha) {
                write_atomic(&target, &theirs, true, &self.ws.settings.failpoints)?;
                self.memo().wrote(path, &target, &theirs, Some(sha));
            }
            crate::tx::remove(pending_path)?;
            self.confirmed.push(attempt_id);
            self.confirmed_events.extend(read_attempt(&target, None));
            return Ok(false);
        }
        let new = if target.exists() {
            // an immutable file that differs from the accepted copy was damaged here
            write_atomic(
                &fork_path(self, &target),
                &read(&target)?,
                true,
                &self.ws.settings.failpoints,
            )?;
            self.forks.push(path.to_owned());
            false
        } else {
            true
        };
        write_atomic(&target, &theirs, true, &self.ws.settings.failpoints)?;
        self.memo().wrote(path, &target, &theirs, Some(sha));
        Ok(new)
    }

    /// One remote segment: ours extended to it (what it lacks appended), or, when neither is a
    /// prefix of the other, a foreign one replaced (ours kept beside it); our own is a fork. How
    /// many events came in. A segment whose published blob is ours is not read: its lines are the
    /// remembered ones.
    fn merge_segment(
        &mut self,
        path: &str,
        sha: &str,
        local: &BTreeMap<String, String>,
        own: Option<&str>,
    ) -> Result<i64, Error> {
        let replica = path.split('/').nth(1).unwrap_or_default();
        let target = self.ws.dir.join(path);
        if local.get(path).map(String::as_str) == Some(sha) {
            let lines = self.memo().lines(path, &target)?;
            self.remote_counts.insert(path.to_owned(), lines);
            return Ok(0);
        }
        let ours = if target.exists() {
            read(&target)?
        } else {
            Vec::new()
        };
        let blob = self.blob(sha)?;
        let theirs = complete_prefix(&blob);
        // what is written is the published blob itself unless an interrupted line was cut from it
        let written = (theirs == blob).then_some(sha);
        self.remote_counts
            .insert(path.to_owned(), newlines(&theirs));
        if theirs == ours || ours.starts_with(&theirs) {
            return Ok(0);
        }
        if theirs.starts_with(&ours) {
            let pulled = newlines(&theirs) as i64 - newlines(&ours) as i64;
            match theirs.get(ours.len()..).filter(|_| !ours.is_empty()) {
                // what ours lacks, appended: the segment stays the file it was (roll-lqjj.2)
                Some(more) => extend_segment(&target, more)?,
                None => write_atomic(&target, &theirs, true, &self.ws.settings.failpoints)?,
            }
            self.memo().wrote(path, &target, &theirs, written);
            return Ok(pulled);
        }
        if Some(replica) == own {
            return Err(Error::JournalFork(OWN_FORK.to_owned()));
        }
        write_atomic(
            &fork_path(self, &target),
            &ours,
            true,
            &self.ws.settings.failpoints,
        )?;
        write_atomic(&target, &theirs, true, &self.ws.settings.failpoints)?;
        self.memo().wrote(path, &target, &theirs, written);
        self.forks.push(path.to_owned());
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_paths_are_told_apart_by_their_shape() {
        assert!(attempt_shaped("events/ab12cd34/claims/0190-ab.json"));
        assert!(!attempt_shaped("events/ab12cd34/claims/0190-AB.json"));
        assert!(!attempt_shaped("events/ab12cd34/claims/.json"));
        assert!(!attempt_shaped("events//claims/x.json"));
        assert!(segment_shaped("events/ab12cd34/000001.jsonl"));
        assert!(!segment_shaped("events/ab12cd34/00a001.jsonl"));
        assert!(!segment_shaped("events/ab12cd34/claims/000001.jsonl"));
        assert_eq!(stem(Path::new("a/0190-ab.json")), "0190-ab");
        assert_eq!(stem(Path::new("a/.json")), ".json");
    }
}
