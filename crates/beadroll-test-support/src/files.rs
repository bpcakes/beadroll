//! Directory trees a test copies or walks: a whole tree copied as it is, and every path under one.

use std::path::{Path, PathBuf};
use std::process::Command;

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
