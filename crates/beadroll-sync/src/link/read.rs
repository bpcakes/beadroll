//! Reads the remote journal through the link's persistent cat-file
//! (`crates/beadroll-git/AGENTS.md`). Resolves the fetch ref, manifest, file listing, blobs and
//! commit tree with the same results as the corresponding git commands. Missing, damaged or
//! unusual answers return NoAnswer and fall back to standalone git with the same error
//! classification.
//!
//! What a fetch brought can be gone from `sync.git` by the time it is read: a prune that began
//! before the fetch's ref existed removes it (roll-dih8.30). A read of the fetched tip that fails
//! asks what of the tip is missing or damaged here ([`Link::lost`]), and that is local damage,
//! which making `sync.git` again repairs, never a remote without a manifest or a failed git. A git
//! a signal ended says nothing either way, and is never taken as damage.

use std::ffi::OsStr;

use beadroll_core::text::strip;
use beadroll_git::{Output, list_tree_with_subtrees, local_damage, root_tree};

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
        match self.lost(tip, Some(&name), &manifest)? {
            Some(lost) => Err(self.damage(&lost)),
            None => Ok(None),
        }
    }

    /// What of the fetched `tip` is missing from `sync.git` or damaged there, `read` having failed
    /// on it, a read of the blob `object` names if there is one: local damage git named, that blob
    /// here but unreadable ([`Link::unreadable`]), the tip's commit missing, or the trees and blobs
    /// it reaches that are missing or that git cannot walk (`rev-list --missing=print`, which
    /// passes over a gitlink as a fetch does; [`walk_failed`], which takes a walk that a full disk
    /// or a memory limit ended for damage too). `None` when everything the tip reaches is here and
    /// walks, and `object` is not a blob here: the read failed for what the remote holds, or for no
    /// reason that is `sync.git`'s, as when a signal ended it.
    pub(super) fn lost(
        &self,
        tip: &str,
        object: Option<&str>,
        read: &Output,
    ) -> Result<Option<String>, Error> {
        let said = String::from_utf8_lossy(&read.stderr);
        let said = strip(&said);
        if local_damage(said) {
            return Ok(Some(said.to_owned()));
        }
        if let Some(object) = object
            && let Some(unreadable) = self.unreadable(object, read)?
        {
            return Ok(Some(unreadable));
        }
        let present = self.run(&[OsStr::new("cat-file"), "-e".as_ref(), tip.as_ref()])?;
        if tip_missing(&present) {
            return Ok(Some(format!("the fetched commit {tip} is missing")));
        }
        if !present.success() {
            return Ok(None);
        }
        let args = ["rev-list", "--objects", "--no-walk", "--missing=print", tip].map(OsStr::new);
        let listed = self.run(&args)?;
        if !listed.success() {
            return Ok(walk_failed(&listed));
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

    /// `object`, whose read as a blob failed as `read`, as damage here when [`blob_unreadable`]
    /// says so. A tree or a commit by that name (manifest.json as a tree or a gitlink, a gitlink
    /// where a segment goes) is the remote's, and so is a name `sync.git` lacks, which
    /// [`Link::lost`] asks about.
    fn unreadable(&self, object: &str, read: &Output) -> Result<Option<String>, Error> {
        let present = self.run(&[OsStr::new("cat-file"), "-e".as_ref(), object.as_ref()])?;
        if !present.success() {
            return Ok(None);
        }
        let kind = self.run(&[OsStr::new("cat-file"), "-t".as_ref(), object.as_ref()])?;
        if !blob_unreadable(read, &kind) {
            return Ok(None);
        }
        let said = String::from_utf8_lossy(&read.stderr);
        Ok(Some(format!(
            "{object} is in sync.git, but git cannot read it: {}",
            strip(&said)
        )))
    }

    /// `read`, the failed `git ARGS` that read what the fetched `tip` holds, the blob `object`
    /// names if there is one: local damage when that is what it was ([`Link::lost`]), else the
    /// GIT_ERROR it is.
    fn read_failed(
        &self,
        tip: &str,
        object: Option<&str>,
        args: &[&OsStr],
        read: &Output,
    ) -> Result<Error, Error> {
        Ok(match self.lost(tip, object, read)? {
            Some(lost) => self.damage(&lost),
            None => read.failed(args).into(),
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
        let listing = self.run(&args)?;
        if !listing.success() {
            return Err(self.read_failed(tip, None, &args, &listing)?);
        }
        let listing = listing.stdout;
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
        let blob = self.run(&args)?;
        if !blob.success() {
            return Err(self.read_failed(tip, Some(sha), &args, &blob)?);
        }
        Ok(blob.stdout)
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

/// Whether `cat-file -e` of a fetched tip, which answered `present`, says `sync.git` lacks it: git
/// ended failing by its own exit, not by a signal.
fn tip_missing(present: &Output) -> bool {
    !present.success() && !present.killed()
}

/// Whether a blob's read that failed as `read`, of an object `sync.git` holds and to which
/// `cat-file -t` answered `kind`, says the blob is damaged here: git ended the read by its own exit
/// and gave the object's type as blob. index-pack verified every object a fetch brought, and bead
/// wrote whole every object it made, so a blob there that git cannot read was damaged here. A
/// read or a `-t` a signal ended, or a `-t` that failed, says nothing; damage git names in either
/// is already its own words.
fn blob_unreadable(read: &Output, kind: &Output) -> bool {
    !read.killed() && kind.success() && kind.stdout_text() == "blob"
}

/// What a `rev-list` of a fetched tip `sync.git` holds that failed as `listed` says: local damage
/// when git ended by its own exit, since no fetch lets in what makes the walk fail (a fetch's own
/// connectivity check walks every object it brings, and refuses a malformed tree); nothing when a
/// signal ended it. A walk git ends by its own exit for a reason that is not the objects', a full
/// disk or a memory limit, is taken as damage too, at the cost of one needless rebuild.
fn walk_failed(listed: &Output) -> Option<String> {
    if listed.killed() {
        return None;
    }
    let said = String::from_utf8_lossy(&listed.stderr);
    let said = strip(&said);
    Some(if said.is_empty() {
        format!("git rev-list ended with {}", listed.returncode)
    } else {
        said.to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A git's output: its return code (minus the signal that ended it) and what it printed.
    fn ended(returncode: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            returncode,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn a_blob_is_unreadable_only_when_git_failed_reading_it_and_types_it_a_blob() {
        let stream = ended(128, "", "fatal: unable to stream 02f5 to stdout");
        let killed = ended(-9, "", ""); // SIGKILL
        let blob = ended(0, "blob\n", "");
        assert!(blob_unreadable(&stream, &blob));
        assert!(!blob_unreadable(&killed, &blob));
        assert!(!blob_unreadable(&stream, &ended(0, "commit\n", "")));
        assert!(!blob_unreadable(&stream, &ended(0, "tree\n", "")));
        assert!(!blob_unreadable(
            &stream,
            &ended(128, "", "fatal: bad header")
        ));
        assert!(!blob_unreadable(&stream, &killed));
    }

    #[test]
    fn a_tip_is_missing_when_git_says_so_by_its_own_exit() {
        assert!(tip_missing(&ended(1, "", "")));
        assert!(!tip_missing(&ended(0, "", "")));
        assert!(!tip_missing(&ended(-9, "", ""))); // SIGKILL
    }

    #[test]
    fn a_failed_walk_is_damage_unless_a_signal_ended_it() {
        let short = ended(128, "", "fatal: too-short tree object\n");
        assert_eq!(
            walk_failed(&short).as_deref(),
            Some("fatal: too-short tree object")
        );
        let silent = walk_failed(&ended(1, "", ""));
        assert_eq!(silent.as_deref(), Some("git rev-list ended with 1"));
        assert_eq!(walk_failed(&ended(-9, "", "")), None); // SIGKILL
    }
}
