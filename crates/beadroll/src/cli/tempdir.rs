//! The temporary directory for files that capture git's output, selected before the first git
//! invocation.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use beadroll_core::code::Code;
use beadroll_core::text::repr_str;

use crate::commands::Failure;

/// The number of candidate filenames tried before giving up on a directory.
const NAMES: usize = 10_000;

/// The first usable temporary directory: `TMPDIR`, `TEMP` and `TMP` when set, then `/tmp`,
/// `/var/tmp` and `/usr/tmp`, then the working directory. A directory is usable when a new file
/// can be created, written and removed there. Returns an error if no candidate is usable.
pub(crate) fn temporary_directory(
    vars: &BTreeMap<OsString, OsString>,
    cwd: Option<&Path>,
) -> Result<PathBuf, Failure> {
    let mut candidates: Vec<OsString> = ["TMPDIR", "TEMP", "TMP"]
        .iter()
        .filter_map(|name| vars.get(OsStr::new(name)))
        .filter(|value| !value.is_empty())
        .cloned()
        .collect();
    candidates.extend(["/tmp", "/var/tmp", "/usr/tmp"].map(OsString::from));
    candidates.push(cwd.map_or_else(|| OsString::from("."), |c| c.as_os_str().to_owned()));
    for candidate in &candidates {
        let dir = if candidate == "." {
            PathBuf::from(".")
        } else {
            abspath(candidate, cwd)
        };
        if usable(&dir) {
            return Ok(dir);
        }
    }
    let shown: Vec<String> = candidates
        .iter()
        .map(|c| repr_str(&c.to_string_lossy()))
        .collect();
    Err(Failure::new(
        Code::Io,
        format!(
            "[Errno 2] No usable temporary directory found in [{}]",
            shown.join(", ")
        ),
    ))
}

/// Whether a new file can be made, written and removed in `dir`, trying fresh names while a name is
/// taken; any other failure gives up on the directory.
fn usable(dir: &Path) -> bool {
    for _ in 0..NAMES {
        let probe = dir.join(beadroll_store::hex_token(8));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&probe);
        match file {
            Ok(mut file) => {
                let written = file.write_all(b"blat");
                drop(file);
                let removed = std::fs::remove_file(&probe);
                return written.is_ok() && removed.is_ok();
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return false,
        }
    }
    false
}

/// `os.path.abspath(path)`: absolute from `cwd`, `.` and `..` resolved and repeated slashes folded,
/// lexically (two leading slashes stay two, as in `posixpath.normpath`).
fn abspath(path: &OsStr, cwd: Option<&Path>) -> PathBuf {
    let bytes = path.as_bytes();
    let joined: Vec<u8> = if bytes.starts_with(b"/") {
        bytes.to_vec()
    } else {
        let base = cwd.map_or(&b"."[..], |c| c.as_os_str().as_bytes());
        [base, b"/", bytes].concat()
    };
    let leading = if joined.starts_with(b"//") && !joined.starts_with(b"///") {
        2
    } else if joined.starts_with(b"/") {
        1
    } else {
        0
    };
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in joined.split(|b| *b == b'/') {
        match part {
            b"" | b"." => {}
            b".." if parts.last().is_some_and(|last| *last != b"..") => {
                parts.pop();
            }
            b".." if leading > 0 => {}
            _ => parts.push(part),
        }
    }
    let mut out = b"/".repeat(leading);
    out.extend(parts.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    PathBuf::from(OsString::from_vec(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_made_absolute_as_python_makes_them() {
        let cwd = Some(Path::new("/w/x"));
        assert_eq!(
            abspath(OsStr::new("t/../u//v/."), cwd),
            Path::new("/w/x/u/v")
        );
        assert_eq!(abspath(OsStr::new("/a/../../b"), cwd), Path::new("/b"));
        assert_eq!(abspath(OsStr::new("//a/b"), cwd), Path::new("//a/b"));
    }

    #[test]
    fn a_missing_tmpdir_falls_back_to_the_next_candidate() {
        let vars: BTreeMap<OsString, OsString> = [
            ("TMPDIR".into(), "/nonexistent/bead-tmp".into()),
            ("TEMP".into(), "".into()),
        ]
        .into_iter()
        .collect();
        let found = temporary_directory(&vars, None).unwrap();
        assert_ne!(found, Path::new("/nonexistent/bead-tmp"));
        assert!(found.is_dir());
    }
}
