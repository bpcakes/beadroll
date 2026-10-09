//! Paths as Python's `os.path` reads them where bead depends on it (`docs/RUST-PORT.md` §5 row 10):
//! `exists` and the lenient, pure-Python `realpath`, ported from CPython 3.14 rather than taken
//! from libc, whose `realpath` fails on a missing component instead of keeping it.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};

/// `os.path.exists(path)`: whether `path` (relative to `cwd`) names something, symlinks followed.
/// The empty path, a path with a NUL byte and one the system cannot look at all answer no.
pub fn exists(path: &OsStr, cwd: &Path) -> bool {
    !path.is_empty() && std::fs::metadata(cwd.join(path)).is_ok()
}

/// `os.path.realpath(path)` (CPython 3.14, not strict): `path` made absolute from `cwd` with every
/// symbolic link resolved, component by component. A component that cannot be looked at, or that
/// is missing, is kept as it is, and so are the ones after it; a symlink loop is left unresolved.
/// `cwd` must be absolute and free of symlinks, as `getcwd()` is.
pub fn realpath(path: &OsStr, cwd: &Path) -> PathBuf {
    let filename = path.as_bytes();
    // The stack of unresolved parts, last part first. `None` marks a symlink whose target has been
    // pushed after it: popping it records the target's resolution under the link's path.
    let mut rest: Vec<Option<Vec<u8>>> = filename
        .split(|b| *b == b'/')
        .rev()
        .map(|part| Some(part.to_vec()))
        .collect();
    let mut part_count = rest.len();
    let mut resolved: Vec<u8> = if filename.starts_with(b"/") {
        b"/".to_vec()
    } else {
        cwd.as_os_str().as_bytes().to_vec()
    };
    // A link's path to its fully resolved target, or None while it is being resolved (a loop).
    let mut seen: HashMap<Vec<u8>, Option<Vec<u8>>> = HashMap::new();
    while part_count > 0 {
        let Some(name) = rest.pop() else { break };
        let Some(name) = name else {
            if let Some(Some(link)) = rest.pop() {
                seen.insert(link, Some(resolved.clone()));
            }
            continue;
        };
        part_count -= 1;
        if name.is_empty() || name == b"." {
            continue;
        }
        if name == b".." {
            resolved = match resolved.iter().rposition(|b| *b == b'/') {
                Some(0) | None => b"/".to_vec(),
                Some(at) => resolved[..at].to_vec(),
            };
            continue;
        }
        let newpath = if resolved == b"/" {
            [&b"/"[..], &name].concat()
        } else {
            [&resolved[..], b"/", &name].concat()
        };
        let as_path = Path::new(OsStr::from_bytes(&newpath));
        let is_link = match std::fs::symlink_metadata(as_path) {
            Ok(meta) => meta.file_type().is_symlink(),
            Err(_) => false, // an error, ignored: the component stays as it is
        };
        if !is_link {
            resolved = newpath;
            continue;
        }
        if let Some(known) = seen.get(&newpath) {
            // Already seen: its resolution, or a loop, left unresolved.
            resolved = known.clone().unwrap_or(newpath);
            continue;
        }
        let Ok(target) = std::fs::read_link(as_path) else {
            resolved = newpath;
            continue;
        };
        let target = target.into_os_string().into_vec();
        if target.starts_with(b"/") {
            resolved = b"/".to_vec();
        }
        seen.insert(newpath.clone(), None);
        rest.push(Some(newpath));
        rest.push(None);
        let parts: Vec<Option<Vec<u8>>> = target
            .split(|b| *b == b'/')
            .rev()
            .map(|part| Some(part.to_vec()))
            .collect();
        part_count += parts.len();
        rest.extend(parts);
    }
    PathBuf::from(OsString::from_vec(resolved))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_parts_are_kept_and_links_resolved() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        std::os::unix::fs::symlink("loop", root.join("loop")).unwrap();
        // Python: `missing` is kept, `..` takes it away again, `x` is kept.
        assert_eq!(
            realpath(OsStr::new("link/missing/../x"), &root),
            root.join("real/x")
        );
        assert_eq!(realpath(OsStr::new("loop"), &root), root.join("loop"));
        assert_eq!(realpath(OsStr::new("/.."), &root), Path::new("/"));
        assert_eq!(realpath(OsStr::new(""), &root), root);
        assert!(exists(OsStr::new("link"), &root));
        assert!(!exists(OsStr::new(""), &root));
        assert!(!exists(OsStr::new("loop"), &root));
    }
}
