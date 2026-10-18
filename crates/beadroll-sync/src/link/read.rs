//! Reads the remote journal through the link's persistent cat-file
//! (`crates/beadroll-git/AGENTS.md`). Resolves the fetch ref, manifest, file listing, blobs and
//! commit tree with the same results as the corresponding git commands. Missing, damaged or
//! unusual answers return NoAnswer and fall back to standalone git with the same error
//! classification.
//!
//! What a fetch brought can be gone from `sync.git` by the time it is read: a prune that began
//! before the fetch's ref existed removes it (roll-dih8.30). A read of the fetched tip that fails
//! asks what of the tip is missing here, or damaged here in git's words or as a blob there but
//! unreadable ([`Link::lost`]), and that is local damage, which making `sync.git` again repairs,
//! never a remote without a manifest or a failed git.

use std::ffi::OsStr;

use beadroll_core::text::strip;
use beadroll_git::{list_tree_with_subtrees, local_damage, root_tree};

use super::Link;
use crate::error::Error;

impl Link<'_> {
    /// The commit named by fetch ref `mine`. A failed rev-parse may indicate local damage.
    pub(super) fn fetched_tip(&mut self, mine: &str) -> Result<String, Error> {
        match self.objects.name(mine) {
            Ok(tip) => Ok(tip),
            Err(_) => self.git(&[OsStr::new("rev-parse"), mine.as_ref()], None),
        }
    }

    /// The manifest.json bytes at `tip`, or `None` if git cannot read them while every object the
    /// tip reaches is in `sync.git` and the blob, if the tree names one, reads: its tree holds no
    /// manifest.json blob. An object of it missing here, or the blob here but unreadable, is local
    /// damage ([`Link::lost`]).
    pub(super) fn manifest(&mut self, tip: &str) -> Result<Option<Vec<u8>>, Error> {
        let name = format!("{tip}:manifest.json");
        if let Ok(object) = self.objects.get(&name)
            && object.kind == "blob"
        {
            return Ok(Some(object.data));
        }
        let manifest = self.run(&[OsStr::new("cat-file"), "blob".as_ref(), name.as_ref()])?;
        if manifest.success() {
            return Ok(Some(manifest.stdout));
        }
        let said = String::from_utf8_lossy(&manifest.stderr);
        match self.lost(tip, Some(&name), strip(&said))? {
            Some(lost) => Err(self.damage(&lost)),
            None => Ok(None),
        }
    }

    /// What of the fetched `tip` is missing from `sync.git` or damaged there, a read of it having
    /// failed with git saying `said`, a read of the blob `object` names if there is one: local
    /// damage git named, that blob here but unreadable ([`Link::unreadable`]), the tip's commit
    /// missing, or the trees and blobs it reaches that are missing (`rev-list --missing=print`,
    /// which passes over a gitlink as a fetch does). `None` when everything the tip reaches is here
    /// and `object` is not a blob here: the read failed for what the remote holds.
    pub(super) fn lost(
        &self,
        tip: &str,
        object: Option<&str>,
        said: &str,
    ) -> Result<Option<String>, Error> {
        if local_damage(said) {
            return Ok(Some(said.to_owned()));
        }
        if let Some(object) = object
            && let Some(unreadable) = self.unreadable(object, said)?
        {
            return Ok(Some(unreadable));
        }
        let present = self.run(&[OsStr::new("cat-file"), "-e".as_ref(), tip.as_ref()])?;
        if !present.success() {
            return Ok(Some(format!("the fetched commit {tip} is missing")));
        }
        let args = ["rev-list", "--objects", "--no-walk", "--missing=print", tip].map(OsStr::new);
        let listed = self.run(&args)?;
        if !listed.success() {
            // only in git's words: a tree the remote sent malformed fails here too, and fetch,
            // without transfer.fsckObjects, does not refuse one
            let said = String::from_utf8_lossy(&listed.stderr);
            let said = strip(&said);
            return Ok(local_damage(said).then(|| said.to_owned()));
        }
        let listed = String::from_utf8_lossy(&listed.stdout);
        let missing: Vec<&str> = listed
            .lines()
            .filter_map(|line| line.strip_prefix('?'))
            .collect();
        if missing.is_empty() {
            return Ok(None);
        }
        Ok(Some(format!(
            "objects the fetched commit {tip} reaches are missing: {}",
            missing.join(", ")
        )))
    }

    /// `object`, whose read as a blob failed with git saying `said`, when `sync.git` holds it as a
    /// blob or as an object git cannot even tell the type of: damaged here, since index-pack
    /// verified every object a fetch brought and bead wrote whole every object it made. A tree or
    /// a commit by that name (manifest.json as a tree or a gitlink, a gitlink where a segment
    /// goes) is the remote's, and so is a name `sync.git` lacks, which [`Link::lost`] asks about.
    fn unreadable(&self, object: &str, said: &str) -> Result<Option<String>, Error> {
        let present = self.run(&[OsStr::new("cat-file"), "-e".as_ref(), object.as_ref()])?;
        if !present.success() {
            return Ok(None);
        }
        let kind = self.run(&[OsStr::new("cat-file"), "-t".as_ref(), object.as_ref()])?;
        if kind.success() && String::from_utf8_lossy(&kind.stdout).trim() != "blob" {
            return Ok(None);
        }
        Ok(Some(format!(
            "{object} is in sync.git, but git cannot read it: {said}"
        )))
    }

    /// `error`, a failed read of what the fetched `tip` holds, the blob `object` names if there is
    /// one, as local damage when that is what it was ([`Link::lost`]).
    fn read_failed(&self, tip: &str, object: Option<&str>, error: Error) -> Result<Error, Error> {
        Ok(match self.lost(tip, object, &error.to_string())? {
            Some(lost) => self.damage(&lost),
            None => error,
        })
    }

    /// Nontree entries under `tip`, with path and object name in git's ls-tree order. Failure
    /// returns GIT_ERROR, or local damage when a tree of the tip is missing or damaged here.
    pub(super) fn listing(&mut self, tip: &str) -> Result<Vec<(Vec<u8>, String)>, Error> {
        if let Ok(commit) = self.objects.get(tip)
            && let Ok(listing) = list_tree_with_subtrees(&mut self.objects, &commit)
        {
            self.listed_trees = listing.subtrees;
            return Ok(listing.files);
        }
        let args = ["ls-tree", "-r", "-z", tip].map(OsStr::new);
        let listing = match self.check(&args) {
            Ok(listing) => listing.stdout,
            Err(error) => return Err(self.read_failed(tip, None, error)?),
        };
        let mut rows = Vec::new();
        for row in listing.split(|b| *b == 0) {
            // `<mode> <type> <object>\t<path>`; -z: the same names whatever core.quotePath says
            let Some(tab) = row.iter().position(|b| *b == b'\t') else {
                continue; // the empty tail
            };
            let meta = String::from_utf8_lossy(&row[..tab]);
            let Some(sha) = meta.split_whitespace().nth(2) else {
                continue;
            };
            rows.push((row[tab + 1..].to_vec(), sha.to_owned()));
        }
        Ok(rows)
    }

    /// The blob named by `sha`, which the fetched `tip` holds; failure returns GIT_ERROR, or local
    /// damage when it is unreadable here or an object of the tip is missing or damaged here.
    pub(super) fn blob(&mut self, tip: &str, sha: &str) -> Result<Vec<u8>, Error> {
        if let Ok(object) = self.objects.get(sha)
            && object.kind == "blob"
        {
            return Ok(object.data);
        }
        let args = ["cat-file", "blob", sha].map(OsStr::new);
        match self.check(&args) {
            Ok(blob) => Ok(blob.stdout),
            Err(error) => Err(self.read_failed(tip, Some(sha), error)?),
        }
    }

    /// The tree of commit `tip`. A failed lookup may indicate local damage.
    pub(super) fn tip_tree(&mut self, tip: &str) -> Result<String, Error> {
        if let Ok(commit) = self.objects.get(tip)
            && let Ok(tree) = root_tree(&mut self.objects, &commit)
        {
            return Ok(tree.oid);
        }
        let current = format!("{tip}^{{tree}}");
        self.git(&[OsStr::new("rev-parse"), current.as_ref()], None)
    }
}
