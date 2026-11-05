//! Evidence that a memoized object is present in this repository. File identity is checked by
//! the memo separately; a remembered name alone does not prove that git still holds its bytes.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub(super) struct KnownObjects {
    made: HashSet<String>,
    listed: HashMap<String, String>,
    trees: HashMap<Vec<u8>, String>,
}

impl KnownObjects {
    pub(super) fn contains_blob(&self, path: &str, blob: &str) -> bool {
        self.made.contains(blob) || self.listed.get(path).is_some_and(|listed| listed == blob)
    }

    pub(super) fn made_blob(&mut self, blob: String) {
        self.made.insert(blob);
    }

    pub(super) fn listed_blob(&mut self, path: String, blob: String) {
        self.listed.insert(path, blob);
    }

    pub(super) fn listed_trees(&mut self, trees: HashMap<Vec<u8>, String>) {
        self.trees = trees;
    }

    pub(super) fn tree(&self, path: &[u8]) -> Option<&String> {
        self.trees.get(path)
    }

    /// Each merge supplies its own reachability evidence, even when it merges no head.
    pub(super) fn before_merge(&mut self) {
        self.listed.clear();
        self.forget_trees();
    }

    /// A repack may prune objects made here unless a published tree reaches them. Rehashing also
    /// discards this evidence before making the blobs again.
    pub(super) fn forget_made(&mut self) {
        self.made.clear();
    }

    pub(super) fn forget_trees(&mut self) {
        self.trees.clear();
    }

    /// No evidence from the repository being set aside applies to its replacement.
    pub(super) fn before_replacement(&mut self) {
        self.forget_made();
        self.before_merge();
    }
}
