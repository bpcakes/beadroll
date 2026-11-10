//! Merges the remote journals into the local store.

mod bootstrap;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use beadroll_core::json::Value;
use beadroll_store::IdentityMode;
use beadroll_store::{FileMode, Sourced};
use beadroll_store::{
    Memo, complete_prefix, extend_segment, pending_files, projection_sourced, read, read_attempt,
    repair_tail, replica_id, segment_files, write_atomic,
};

use super::{Link, newlines};
use crate::error::{Error, OWN_CHAIN};
use crate::link::write::BlobUse;
use crate::synchronise::SyncReport;
use crate::tx::Spent;

const OWN_FORK: &str = "this machine's journal differs from the published one: the same machine \
                        identity is being used from two places. Give one of them a new identity.";

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

/// The filename without its final suffix.
fn stem(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match name.rfind('.') {
        Some(at) if at > 0 => name[..at].to_owned(),
        _ => name.into_owned(),
    }
}

/// A completed merge owns the checked state and the facts from that same round.
pub(crate) struct Merged {
    pub state: Sourced,
    pub pulled: i64,
    pub facts: MergeFacts,
    // Last field: observes destruction after the unused sourced projection, in unit tests only.
    #[cfg(test)]
    _drop_probe: Option<crate::tx::DropProbe>,
}

#[derive(Default)]
pub(crate) struct MergeFacts {
    pub remote_counts: HashMap<String, usize>,
    pub forks: Vec<String>,
    pub confirmed: Vec<String>,
    pub confirmed_events: Vec<Value>,
}

/// Only a running merge can hold partial facts or remember journal writes.
struct Merging<'l, 'w> {
    link: &'l mut Link<'w>,
    facts: MergeFacts,
    wrote: bool,
}

impl Link<'_> {
    /// Brings in the journals at `tip`, checking this machine's chain, and returns the number of
    /// events received. Foreign forks are preserved beside their published replacements; an own
    /// fork fails. Pending attempts found published are confirmed. Call with the store lock held.
    /// The public report fields retain this operation's facts, including progress before an error.
    /// Keep this link outside the lock scope: it retains the completed view until its next merge
    /// or destruction, so freeing a large projection need not hold up other writers.
    pub fn merge(&mut self, tip: Option<&str>) -> Result<i64, Error> {
        let mut merging = Merging::new(self);
        let result = merging.run(tip, SyncReport::Skip);
        let facts = merging.facts;
        self.remote_counts = facts.remote_counts;
        self.forks = facts.forks;
        self.confirmed = facts.confirmed;
        self.confirmed_events = facts.confirmed_events;
        let (state, pulled) = result?;
        self.retained = Spent::from_projection(state.into_projection());
        Ok(pulled)
    }

    /// Internal rounds receive a completed merge rather than reading report slots on the link.
    /// The first requested rename count remains command-wide, even across a failed round.
    pub(crate) fn merge_from(
        &mut self,
        tip: Option<&str>,
        count: SyncReport,
    ) -> Result<Merged, Error> {
        let mut merging = Merging::new(self);
        let (state, pulled) = merging.run(tip, count)?;
        Ok(Merged {
            state,
            pulled,
            facts: merging.facts,
            #[cfg(test)]
            _drop_probe: drop_tests::probe(self.ws),
        })
    }
}

impl<'l, 'w> Merging<'l, 'w> {
    fn new(link: &'l mut Link<'w>) -> Self {
        link.retained = Spent::default();
        Self {
            link,
            facts: MergeFacts::default(),
            wrote: false,
        }
    }

    fn run(&mut self, tip: Option<&str>, count: SyncReport) -> Result<(Sourced, i64), Error> {
        let count = matches!(count, SyncReport::Collect);
        // the memo as it is now: another bead process may have written it since this link last did
        self.link.memo = Some(Memo::load(self.link.ws));
        self.link.known.before_merge();
        let mut pulled: i64 = 0;
        for path in segment_files(self.link.ws)? {
            repair_tail(self.link.ws, &path)?;
        }
        // the store's projection, with every whole line its segments hold
        let found = if count && self.link.renames_before.is_none() {
            let found = projection_sourced(self.link.ws)?;
            self.link.renames_before = Some(found.state().renames().len());
            Some(found)
        } else {
            None
        };
        let own = replica_id(&self.link.ws.settings, IdentityMode::Existing)?;
        let pending: HashMap<String, PathBuf> = pending_files(self.link.ws)
            .into_iter()
            .map(|path| (stem(&path), path))
            .collect();
        if let Some(tip) = tip {
            let local = self.link.local_blobs(BlobUse::Comparison)?;
            let listing = self.link.listing(tip)?;
            if local.is_empty() && pending.is_empty() && self.can_bootstrap(&listing) {
                pulled += self.bootstrap(tip, &listing)?;
            } else {
                for (path, sha) in listing {
                    let Ok(path) = String::from_utf8(path) else {
                        continue; // a name no bead writes
                    };
                    self.link.known.listed_blob(path.clone(), sha.clone());
                    if attempt_shaped(&path) {
                        if self.merge_attempt(tip, &path, &sha, &local, &pending)? {
                            pulled += 1;
                        }
                        continue;
                    }
                    if segment_shaped(&path) {
                        pulled += self.merge_segment(tip, &path, &sha, &local, own.as_deref())?;
                    }
                }
            }
        }
        self.link.save_memo();
        // the store's projection, the records merged applied to it, or the journal replayed
        let merged = match found {
            Some(found) if !self.wrote => found,
            _ => projection_sourced(self.link.ws)?,
        };
        if let Some(own) = &own
            && merged.state().chain_breaks().contains_key(own)
        {
            return Err(Error::JournalFork(OWN_CHAIN.to_owned()));
        }
        Ok((merged, pulled))
    }

    /// Writes `data` to the journal file at `target` (`path` under the store) as published: the
    /// memo remembers it, and the merge has written.
    fn write_journal(
        &mut self,
        path: &str,
        target: &Path,
        data: &[u8],
        blob: Option<&str>,
    ) -> Result<(), Error> {
        write_atomic(
            target,
            data,
            FileMode::Private,
            &self.link.ws.settings.failpoints,
        )?;
        self.wrote(path, target, data, blob);
        Ok(())
    }

    /// The journal file at `target` (`path` under the store) was written with `data`: the memo
    /// remembers it, and the merge has written.
    fn wrote(&mut self, path: &str, target: &Path, data: &[u8], blob: Option<&str>) {
        self.wrote = true;
        self.link.memo().wrote(path, target, data, blob);
    }

    /// One remote attempt file at `tip`: confirms a pending attempt of ours, copies a new one, or
    /// replaces a local copy that differs (kept beside it). Whether it was new here.
    fn merge_attempt(
        &mut self,
        tip: &str,
        path: &str,
        sha: &str,
        local: &BTreeMap<String, String>,
        pending: &HashMap<String, PathBuf>,
    ) -> Result<bool, Error> {
        self.facts.remote_counts.insert(path.to_owned(), 1);
        let attempt_id = stem(Path::new(path));
        let target = self.link.ws.dir.join(path);
        let ours_pending = pending.get(&attempt_id);
        if local.get(path).map(String::as_str) == Some(sha) && ours_pending.is_none() {
            return Ok(false);
        }
        let theirs = self.link.blob(tip, sha)?;
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
                self.write_journal(path, &target, &theirs, Some(sha))?;
                self.link.ws.forget_claims(&target);
            }
            crate::tx::remove(pending_path)?;
            self.facts.confirmed.push(attempt_id);
            self.facts
                .confirmed_events
                .extend(read_attempt(&target, None));
            return Ok(false);
        }
        let new = if target.exists() {
            // an immutable file that differs from the accepted copy was damaged here
            write_atomic(
                &fork_path(self.link, &target),
                &read(&target)?,
                FileMode::Private,
                &self.link.ws.settings.failpoints,
            )?;
            self.facts.forks.push(path.to_owned());
            false
        } else {
            true
        };
        self.write_journal(path, &target, &theirs, Some(sha))?;
        self.link.ws.forget_claims(&target);
        Ok(new)
    }

    /// One remote segment at `tip`: ours extended to it (what it lacks appended), or, when neither
    /// is a prefix of the other, a foreign one replaced (ours kept beside it); our own is a fork.
    /// How many events came in. A segment whose published blob is ours is not read: its lines are
    /// the remembered ones.
    fn merge_segment(
        &mut self,
        tip: &str,
        path: &str,
        sha: &str,
        local: &BTreeMap<String, String>,
        own: Option<&str>,
    ) -> Result<i64, Error> {
        let replica = path.split('/').nth(1).unwrap_or_default();
        let target = self.link.ws.dir.join(path);
        if local.get(path).map(String::as_str) == Some(sha) {
            let lines = self.link.memo().lines(path, &target)?;
            self.facts.remote_counts.insert(path.to_owned(), lines);
            return Ok(0);
        }
        let ours = if target.exists() {
            read(&target)?
        } else {
            Vec::new()
        };
        let blob = self.link.blob(tip, sha)?;
        let theirs = complete_prefix(&blob);
        // what is written is the published blob itself unless an interrupted line was cut from it
        let written = (theirs == blob).then_some(sha);
        self.facts
            .remote_counts
            .insert(path.to_owned(), newlines(&theirs));
        if theirs == ours || ours.starts_with(&theirs) {
            return Ok(0);
        }
        if theirs.starts_with(&ours) {
            let pulled = newlines(&theirs) as i64 - newlines(&ours) as i64;
            match theirs.get(ours.len()..).filter(|_| !ours.is_empty()) {
                // what ours lacks, appended: the segment stays the file it was (roll-lqjj.2)
                Some(more) => {
                    extend_segment(&target, more)?;
                    self.wrote(path, &target, &theirs, written);
                }
                None => self.write_journal(path, &target, &theirs, written)?,
            }
            return Ok(pulled);
        }
        if Some(replica) == own {
            return Err(Error::JournalFork(OWN_FORK.to_owned()));
        }
        write_atomic(
            &fork_path(self.link, &target),
            &ours,
            FileMode::Private,
            &self.link.ws.settings.failpoints,
        )?;
        self.write_journal(path, &target, &theirs, written)?;
        self.facts.forks.push(path.to_owned());
        Ok(0)
    }
}

#[cfg(test)]
mod drop_tests;
#[cfg(test)]
mod tests;
