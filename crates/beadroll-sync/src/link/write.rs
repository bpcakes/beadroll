//! Writing into `sync.git` through the link's long-lived gits (`docs/RUST-PORT.md` Appendix B):
//! blobs through one `hash-object` between fetches, the trees of a publish through one `mktree
//! --batch`, `refs/beads/remote` after a push through an `update-ref --stdin` transaction. Each
//! writes what the one-shot command Python runs writes; whenever a kept git has no answer
//! ([`beadroll_git::NoAnswer`]), that command is run instead, and its answer or failure is the one
//! given. The refs after a fetch are Python's own two calls ([`Link::keep_fetched`]).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use beadroll_git::{Hasher, NoAnswer, Refs, Trees};
use beadroll_store::{REMOTE_REF, claim_files, relative, segment_files};

use super::Link;
use crate::error::Error;

impl Link<'_> {
    /// Every local journal file in `sync.git` as a blob, by its path in the store (Python's
    /// `hash-object -w --no-filters --stdin-paths`, whose failure may be damage).
    pub(super) fn local_blobs(&mut self) -> Result<BTreeMap<String, String>, Error> {
        let mut files = segment_files(self.ws)?;
        files.extend(claim_files(self.ws)?);
        if files.is_empty() {
            return Ok(BTreeMap::new());
        }
        let paths: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
        let shas = match self.hasher().and_then(|hasher| hasher.hash(&paths)) {
            Ok(shas) => shas,
            Err(NoAnswer) => {
                self.hasher = None;
                let mut input = Vec::new();
                for file in &files {
                    input.extend_from_slice(file.as_os_str().as_bytes());
                    input.push(b'\n');
                }
                let args = ["hash-object", "-w", "--no-filters", "--stdin-paths"].map(OsStr::new);
                let printed = self.git(&args, Some(input))?;
                printed.split_whitespace().map(str::to_owned).collect()
            }
        };
        Ok(files
            .iter()
            .zip(shas)
            .map(|(file, sha)| (relative(self.ws, file), sha))
            .collect())
    }

    /// `data` as a blob in `sync.git` (Python's `hash-object -w --stdin`, whose failure may be
    /// damage).
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

    /// The tree `entries` make (Python's `mktree`, whose failure may be damage): through
    /// `trees`, this publish's `mktree --batch`, while it answers.
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

    /// After a fetch into `mine` of `tip`: `refs/beads/remote` at `tip`, keeping what was fetched
    /// reachable, then `mine` gone, by Python's two `update-ref`s, whose failures it passes over.
    ///
    /// Not as one transaction of the link's `update-ref`, which would delete `mine` within a
    /// millisecond of the fetch. The fetches other bead processes of this store started with this
    /// one are then still checking what they brought against every ref (`rev-list --not --all`),
    /// and one that lists `mine` and finds it gone when it reads it fails: git takes a ref that
    /// vanishes while it lists them for a broken one (`bad object refs/beads/fetch/...`), and their
    /// command for REMOTE_UNREACHABLE. Python's two calls take long enough for those checks to be
    /// over (G16).
    pub(super) fn keep_fetched(&self, tip: &str, mine: &str) -> Result<(), Error> {
        self.run(&[OsStr::new("update-ref"), REMOTE_REF.as_ref(), tip.as_ref()])?;
        self.run(&[OsStr::new("update-ref"), "-d".as_ref(), mine.as_ref()])?;
        Ok(())
    }

    /// Starts the link's `update-ref` if it has none, once a publish knows it will push: it is
    /// ready when the push is done.
    pub(super) fn ready_refs(&mut self) {
        if self.refs.is_none() {
            self.refs = Refs::start(&self.env).ok();
        }
    }

    /// `refs/beads/remote` at `commit`, which the remote has just taken (Python's `update-ref`,
    /// whose failure may be damage).
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

    /// `commands` as one transaction of the link's `update-ref`, started now if it was not before
    /// the push; one that does not go through ends it.
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
