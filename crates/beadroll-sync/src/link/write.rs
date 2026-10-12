//! Writing into `sync.git` through the link's long-lived gits (`docs/RUST-PORT.md` Appendix B):
//! blobs through one `hash-object` between fetches, of the journal files the memo cannot vouch
//! for; the trees of a publish through one `mktree --batch`; `refs/beads/remote` after a push
//! through an `update-ref --stdin` transaction. Each writes what the one-shot command Python runs
//! writes; whenever a kept git has no answer ([`beadroll_git::NoAnswer`]), that command is run
//! instead, and its answer or failure is the one given. The refs after a fetch are Python's own two
//! calls ([`Link::keep_fetched`]).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use beadroll_git::{Hasher, NoAnswer, Refs, Trees};
use beadroll_store::{FileKey, Memo, REMOTE_REF, claim_files, relative, segment_files};

use super::Link;
use crate::error::Error;

impl Link<'_> {
    /// Every local journal file as a blob, by its path in the store. A file keeps the blob the memo
    /// remembers for it while it has the identity it was hashed at; for a tree (`for_tree`), only
    /// when the blob is also sure to be in `sync.git`: this link made it, or the last merged tip
    /// holds it at that path, where a blob no ref reaches may have gone with a gc. A merge only
    /// compares the blob with the published one, which no other blob equals. Every other file is
    /// hashed (Python's `hash-object -w --no-filters --stdin-paths`, whose failure may be damage)
    /// and remembered. Lock held.
    pub(super) fn local_blobs(
        &mut self,
        for_tree: bool,
    ) -> Result<BTreeMap<String, String>, Error> {
        let ws = self.ws;
        let mut files = segment_files(ws)?;
        files.extend(claim_files(ws)?);
        let names: Vec<String> = files.iter().map(|file| relative(ws, file)).collect();
        let present: BTreeSet<&str> = names.iter().map(String::as_str).collect();
        let memo = self.memo.get_or_insert_with(|| Memo::load(ws));
        memo.retain(|name| present.contains(name));
        let mut blobs = BTreeMap::new();
        let mut unknown = Vec::new();
        for (file, name) in files.iter().zip(&names) {
            let key = FileKey::of(file);
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

    /// The files at `paths` as blobs in `sync.git`, in order: through the link's `hash-object`,
    /// else Python's, whose failure may be damage.
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
