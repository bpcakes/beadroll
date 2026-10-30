//! Writes to sync.git through persistent git processes: hash-object between fetches for files the
//! memo cannot validate, mktree per publication and update-ref transactions after fetches and
//! pushes. Unusable answers fall back to standalone commands.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use beadroll_git::{Hasher, NoAnswer, Refs, Trees};
use beadroll_store::{FileKey, Memo, REMOTE_REF, claim_files, claim_keys, relative, segment_files};

use super::Link;
use crate::error::Error;

/// What local journal blob names are needed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlobUse {
    /// Comparison.
    Comparison,
    /// Publication.
    Publication,
}

impl Link<'_> {
    /// Local journal blobs by store path, under the lock. Reuse a memoized hash while the file
    /// identity matches: an attempt file's is the one taken in this locked phase, each stat-ed
    /// once whoever asks ([`claim_keys`]). A publication tree additionally requires the blob to
    /// exist in sync.git: this link created it or the last merged tree references it at this
    /// path, protecting against garbage collection. Merge comparisons need only the hash. Hash and
    /// remember other files; failures may indicate local damage.
    pub(super) fn local_blobs(
        &mut self,
        for_tree: BlobUse,
    ) -> Result<BTreeMap<String, String>, Error> {
        let for_tree = matches!(for_tree, BlobUse::Publication);
        let ws = self.ws;
        let segments = segment_files(ws)?;
        let attempts = claim_files(ws)?;
        let keys = (segments.iter().map(|file| FileKey::of(file)))
            .chain(claim_keys(ws, &attempts).iter().copied())
            .collect::<Vec<_>>();
        let files: Vec<&PathBuf> = segments.iter().chain(attempts.iter()).collect();
        let names: Vec<String> = files.iter().map(|file| relative(ws, file)).collect();
        let present: BTreeSet<&str> = names.iter().map(String::as_str).collect();
        let memo = self.memo.get_or_insert_with(|| Memo::load(ws));
        memo.retain(|name| present.contains(name));
        let mut blobs = BTreeMap::new();
        let mut unknown = Vec::new();
        for ((file, name), key) in files.iter().zip(&names).zip(keys) {
            let remembered = key
                .as_ref()
                .and_then(|key| memo.blob(name, key))
                .filter(|blob| {
                    !for_tree
                        || self.made.contains(*blob)
                        || self.listed.get(name).is_some_and(|listed| listed == blob)
                });
            match remembered {
                Some(blob) => {
                    blobs.insert(name.clone(), blob.to_owned());
                }
                None => unknown.push((file.as_path(), name, key)),
            }
        }
        if unknown.is_empty() {
            return Ok(blobs);
        }
        let paths: Vec<&Path> = unknown.iter().map(|(file, ..)| *file).collect();
        let shas = self.hash_files(&paths)?;
        for ((_, name, key), sha) in unknown.into_iter().zip(shas) {
            self.hashed.push(name.clone());
            if let Some(key) = key {
                self.memo().hashed(name, key, &sha);
            }
            self.made.insert(sha.clone());
            blobs.insert(name.clone(), sha);
        }
        Ok(blobs)
    }

    /// Blobs for `paths`, in order, through persistent hash-object or standalone fallback.
    /// Failures may indicate local damage.
    fn hash_files(&mut self, paths: &[&Path]) -> Result<Vec<String>, Error> {
        match self.hasher().and_then(|hasher| hasher.hash(paths)) {
            Ok(shas) => Ok(shas),
            Err(NoAnswer) => {
                self.hasher = None;
                let mut input = Vec::new();
                for path in paths {
                    input.extend_from_slice(path.as_os_str().as_bytes());
                    input.push(b'\n');
                }
                let args = ["hash-object", "-w", "--no-filters", "--stdin-paths"].map(OsStr::new);
                let printed = self.git(&args, Some(input))?;
                Ok(printed.split_whitespace().map(str::to_owned).collect())
            }
        }
    }

    /// Writes `data` as a blob using hash-object -w --stdin. Failure may indicate local damage.
    pub(super) fn blob_of(&mut self, data: Vec<u8>) -> Result<String, Error> {
        let tmp = self.env.tmp().to_path_buf();
        match self
            .hasher()
            .and_then(|hasher| hasher.hash_data(&data, &tmp))
        {
            Ok(sha) => Ok(sha),
            Err(NoAnswer) => {
                self.hasher = None;
                let args = ["hash-object", "-w", "--stdin"].map(OsStr::new);
                self.git(&args, Some(data))
            }
        }
    }

    /// The link's `hash-object`, started when first needed after a fetch.
    fn hasher(&mut self) -> Result<&mut Hasher, NoAnswer> {
        match &mut self.hasher {
            Some(hasher) => Ok(hasher),
            slot @ None => Ok(slot.insert(Hasher::start(&self.env)?)),
        }
    }

    /// Creates the tree from `entries` through this publish's mktree batch process while
    /// available. Failure may indicate local damage.
    pub(super) fn make_tree(
        &self,
        trees: &mut Option<Trees>,
        entries: String,
    ) -> Result<String, Error> {
        if let Some(batch) = trees {
            match batch.make(entries.as_bytes()) {
                Ok(tree) => return Ok(tree),
                Err(NoAnswer) => *trees = None,
            }
        }
        self.git(&[OsStr::new("mktree")], Some(entries.into_bytes()))
    }

    /// After fetching `tip` into `mine`, update refs/beads/remote to keep the fetched objects
    /// reachable and delete `mine` in the same transaction. Ignore failures of these bookkeeping
    /// calls, falling back to separate invocations if the transaction cannot answer.
    ///
    /// Deleting the fetch ref can race another fetch's check of all refs. That fetch retries
    /// (`fetch.retry`), including when this transaction removes the ref sooner than two calls did.
    pub(super) fn keep_fetched(&mut self, tip: &str, mine: &str) -> Result<(), Error> {
        if self
            .transaction(&[
                format!("update {REMOTE_REF} {tip}"),
                format!("delete {mine}"),
            ])
            .is_ok()
        {
            return Ok(());
        }
        self.run(&[OsStr::new("update-ref"), REMOTE_REF.as_ref(), tip.as_ref()])?;
        self.run(&[OsStr::new("update-ref"), "-d".as_ref(), mine.as_ref()])?;
        Ok(())
    }

    /// Starts the link's `update-ref` if the fetch did not, once a publish knows it will push:
    /// it is ready when the push is done.
    pub(super) fn ready_refs(&mut self) {
        if self.refs.is_none() {
            self.refs = Refs::start(&self.env).ok();
        }
    }

    /// Updates refs/beads/remote to the accepted commit. Failure may indicate local damage.
    pub(super) fn keep_published(&mut self, commit: &str) -> Result<(), Error> {
        if self
            .transaction(&[format!("update {REMOTE_REF} {commit}")])
            .is_ok()
        {
            return Ok(());
        }
        let args = ["update-ref", REMOTE_REF, commit].map(OsStr::new);
        self.git(&args, None).map(drop)
    }

    /// `commands` as one transaction of the link's `update-ref`, started when first needed;
    /// one that does not go through ends it.
    fn transaction(&mut self, commands: &[String]) -> Result<(), NoAnswer> {
        let refs = match &mut self.refs {
            Some(refs) => refs,
            slot @ None => slot.insert(Refs::start(&self.env)?),
        };
        let applied = refs.apply(commands);
        if applied.is_err() {
            self.refs = None;
        }
        applied
    }
}
