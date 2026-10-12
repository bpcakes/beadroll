//! Reads the remote journal through the link's persistent cat-file (`docs/RUST-PORT.md` Appendix
//! B). Resolves the fetch ref, manifest, file listing, blobs and commit tree with the same
//! results as the corresponding git commands. Missing, damaged or unusual answers return NoAnswer
//! and fall back to standalone git with the same error classification.

use std::ffi::OsStr;

use beadroll_git::{list_tree, root_tree};

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

    /// The manifest.json bytes at `tip`, or `None` if git cannot read them.
    pub(super) fn manifest(&mut self, tip: &str) -> Result<Option<Vec<u8>>, Error> {
        let name = format!("{tip}:manifest.json");
        if let Ok(object) = self.objects.get(&name)
            && object.kind == "blob"
        {
            return Ok(Some(object.data));
        }
        let manifest = self.run(&[OsStr::new("cat-file"), "blob".as_ref(), name.as_ref()])?;
        Ok(manifest.success().then_some(manifest.stdout))
    }

    /// Nontree entries under `tip`, with path and object name in git's ls-tree order. Failure
    /// returns GIT_ERROR.
    pub(super) fn listing(&mut self, tip: &str) -> Result<Vec<(Vec<u8>, String)>, Error> {
        if let Ok(commit) = self.objects.get(tip)
            && let Ok(rows) = list_tree(&mut self.objects, &commit)
        {
            return Ok(rows);
        }
        let args = ["ls-tree", "-r", "-z", tip].map(OsStr::new);
        let listing = self.check(&args)?.stdout;
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

    /// The blob named by `sha`; failure returns GIT_ERROR.
    pub(super) fn blob(&mut self, sha: &str) -> Result<Vec<u8>, Error> {
        if let Ok(object) = self.objects.get(sha)
            && object.kind == "blob"
        {
            return Ok(object.data);
        }
        let args = ["cat-file", "blob", sha].map(OsStr::new);
        Ok(self.check(&args)?.stdout)
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
