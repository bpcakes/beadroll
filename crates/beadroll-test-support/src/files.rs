//! Directory trees a test copies or walks: a whole tree copied as it is, every path under one, and
//! the journal segments of a store or of one machine's journal.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The journal segments in the folder `dir`, its `*.jsonl`, sorted by name: one machine's journal.
/// A folder that cannot be read fails the test, never passing for one with no segment.
pub fn segments_in(dir: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("a journal's folder {}: {error}", dir.display()));
    let mut found: Vec<PathBuf> = entries
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.is_file() && path.as_os_str().as_encoded_bytes().ends_with(b".jsonl"))
        .collect();
    found.sort();
    found
}

/// Every journal segment in the store `store`, `events/*/*.jsonl`, sorted by path.
pub fn segments(store: &Path) -> Vec<PathBuf> {
    let events = store.join("events");
    let replicas = std::fs::read_dir(&events)
        .unwrap_or_else(|error| panic!("a store's journals {}: {error}", events.display()));
    let mut found: Vec<PathBuf> = (replicas.map(|entry| entry.expect("a directory entry").path()))
        .filter(|path| path.is_dir())
        .flat_map(|replica| segments_in(&replica))
        .collect();
    found.sort();
    found
}

/// `to`, which must not exist yet, becomes a copy of the directory `from`, its files' modes and
/// times kept (`cp -pR`): a machine's home cloned to make a twin, say.
pub fn copy_tree(from: &Path, to: &Path) {
    assert!(!to.exists(), "{} exists", to.display());
    let status = Command::new("cp")
        .arg("-pR")
        .arg(from)
        .arg(to)
        .status()
        .expect("cp starts");
    assert!(status.success(), "cp -pR {} failed", from.display());
}

/// Every path under `dir`, `dir` itself not among them: depth first, each directory before what
/// it holds, symbolic links listed and not followed.
pub fn descendants(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    gather(dir, &mut found);
    found
}

/// [`descendants`] of `dir`, appended to `found`.
fn gather(dir: &Path, found: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        found.push(path.clone());
        let meta = std::fs::symlink_metadata(&path).expect("the entry's metadata");
        if meta.is_dir() {
            gather(&path, found);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{copy_tree, descendants};

    /// A copy holds what its source does; a walk lists each directory before what it holds and a
    /// link without what it points at.
    #[test]
    fn a_tree_copied_is_walked_as_its_source() {
        let root = tempfile::tempdir().expect("a scratch directory");
        let source = root.path().join("a");
        std::fs::create_dir_all(source.join("b")).unwrap();
        std::fs::write(source.join("b/c"), b"c").unwrap();
        std::os::unix::fs::symlink(root.path(), source.join("up")).unwrap();
        let copy = root.path().join("copy");
        copy_tree(&source, &copy);
        let mut walked: Vec<_> = descendants(&copy)
            .into_iter()
            .map(|path| {
                path.strip_prefix(&copy)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let at = |name: &str| walked.iter().position(|seen| seen == name).unwrap();
        assert!(at("b") < at("b/c"), "{walked:?}");
        walked.sort();
        assert_eq!(walked, ["b", "b/c", "up"]);
        assert_eq!(std::fs::read(copy.join("b/c")).unwrap(), b"c");
    }
}
