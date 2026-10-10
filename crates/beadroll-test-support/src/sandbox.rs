//! The sandbox: a temporary root with a local bare remote, and the `bead`s its machines run.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use tempfile::TempDir;

/// The variable that names the `bead` under test, as in `tests/support.py`.
pub const UNDER_TEST: &str = "BEAD_UNDER_TEST";

/// The variable that names a second `bead`, for a fleet that mixes implementations
/// (`docs/RUST-PORT.md` §4.2, item 4): set and not empty, it gives that `bead` to every second
/// machine a sandbox makes, the second, the fourth and so on in the order they are made, unless a
/// machine has its own; the others run the `bead` under test. So tests written for one binary run
/// a mixed fleet, one machine of each implementation when there are two.
pub const MIXED_WITH: &str = "BEAD_MIXED_WITH";

/// The `bead` a sandbox's machines run: `under_test` (the value of `BEAD_UNDER_TEST`) when it is
/// set and not empty, as `support.py` reads it, else `default`. Both must be absolute paths,
/// because the tests run `bead` from many directories; anything else panics.
pub fn select_bead(under_test: Option<&OsStr>, default: &Path) -> PathBuf {
    let (path, source) = match under_test.filter(|value| !value.is_empty()) {
        Some(value) => (PathBuf::from(value), UNDER_TEST),
        None => (default.to_path_buf(), "the default bead"),
    };
    assert!(
        path.is_absolute(),
        "{source} must be an absolute path, not {}",
        path.display()
    );
    path
}

/// The `bead` every second machine of a sandbox runs: `mixed_with` (the value of
/// `BEAD_MIXED_WITH`) when it is set and not empty, else none, and every machine runs the
/// sandbox's own. It must be an absolute path, as [`select_bead`]'s are; anything else panics.
pub fn select_mixed(mixed_with: Option<&OsStr>) -> Option<PathBuf> {
    let path = PathBuf::from(mixed_with.filter(|value| !value.is_empty())?);
    assert!(
        path.is_absolute(),
        "{MIXED_WITH} must be an absolute path, not {}",
        path.display()
    );
    Some(path)
}

/// One test's world, as `support.Sandbox` builds it: a temporary root holding `origin.git`, a bare
/// remote whose `main` has one commit. Dropping it removes the root, after making everything in it
/// writable again, since tests make stores read-only.
pub struct Sandbox {
    root: PathBuf,
    origin: PathBuf,
    bead: PathBuf,
    /// The `bead` every second machine runs, when `BEAD_MIXED_WITH` names one.
    mixed: Option<PathBuf>,
    /// How many machines have been made here: whether the next is a second one. Atomic, because
    /// a test may make its machines from several threads.
    made: AtomicUsize,
    // Removes the root when the sandbox drops, after `Drop for Sandbox` has made it writable.
    _dir: TempDir,
}

impl Sandbox {
    /// A new sandbox whose machines run `default_bead`, unless `BEAD_UNDER_TEST` names another,
    /// `BEAD_MIXED_WITH` names one for every second machine, or a machine has its own
    /// ([`crate::Machine::with_bead`]).
    pub fn new(default_bead: impl AsRef<Path>) -> Sandbox {
        let under_test = std::env::var_os(UNDER_TEST);
        let mixed_with = std::env::var_os(MIXED_WITH);
        Sandbox::selecting(default_bead, under_test.as_deref(), mixed_with.as_deref())
    }

    /// A new sandbox as [`Sandbox::new`] makes it, with the values of `BEAD_UNDER_TEST` and
    /// `BEAD_MIXED_WITH` given instead of read from the environment, which a test cannot change
    /// for itself alone: for the tests of the selection.
    pub fn selecting(
        default_bead: impl AsRef<Path>,
        under_test: Option<&OsStr>,
        mixed_with: Option<&OsStr>,
    ) -> Sandbox {
        let bead = select_bead(under_test, default_bead.as_ref());
        let mixed = select_mixed(mixed_with);
        let dir = tempfile::Builder::new()
            .prefix("bead-test-")
            .tempdir()
            .expect("a temporary directory for the sandbox");
        // Resolved, as `Path.resolve()` resolves it in Python: on macOS the temporary directory is
        // reached through a symlink, and the tests compare paths that bead prints.
        let root = dir.path().canonicalize().expect("the sandbox's own path");
        let sandbox = Sandbox {
            origin: root.join("origin.git"),
            root,
            bead,
            mixed,
            made: AtomicUsize::new(0),
            _dir: dir,
        };
        sandbox.bare("origin.git");
        let seed = sandbox.clone("seed", None);
        std::fs::write(seed.join("README.md"), "example project\n").expect("the seed's README");
        git(["add", "-A"], Some(&seed));
        let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
        git(
            identity.into_iter().chain(["commit", "-q", "-m", "init"]),
            Some(&seed),
        );
        git(["push", "-q", "origin", "HEAD:main"], Some(&seed));
        sandbox
    }

    /// The sandbox's root directory, symlinks resolved.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The bare remote every machine clones, `origin.git`.
    pub fn origin(&self) -> &Path {
        &self.origin
    }

    /// The `bead` this sandbox's machines run unless they have their own, or are every second
    /// machine of a mixed fleet ([`Sandbox::mixed_with`]).
    pub fn bead(&self) -> &Path {
        &self.bead
    }

    /// The second `bead`, which every second machine made here runs unless it has its own: the
    /// one `BEAD_MIXED_WITH` names, if it names one.
    pub fn mixed_with(&self) -> Option<&Path> {
        self.mixed.as_deref()
    }

    /// The `bead` the next machine made here runs unless it has its own: the second `bead` if
    /// there is one and the machine is the second, the fourth and so on, else [`Sandbox::bead`].
    pub(crate) fn next_bead(&self) -> PathBuf {
        let made_before = self.made.fetch_add(1, Ordering::Relaxed);
        match &self.mixed {
            Some(second) if made_before % 2 == 1 => second.clone(),
            _ => self.bead.clone(),
        }
    }

    /// A new bare repository `name` in the sandbox, whose default branch is `main`.
    pub fn bare(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        let args: [&OsStr; 5] = [
            "init".as_ref(),
            "-q".as_ref(),
            "--bare".as_ref(),
            "-b".as_ref(),
            "main".as_ref(),
        ];
        git(args.into_iter().chain([path.as_os_str()]), None);
        path
    }

    /// A clone `name` in the sandbox of `origin`, or of `origin.git` when that is `None`.
    pub fn clone(&self, name: &str, origin: Option<&Path>) -> PathBuf {
        let path = self.root.join(name);
        let from = origin.unwrap_or(&self.origin);
        let args: [&OsStr; 2] = ["clone".as_ref(), "-q".as_ref()];
        git(
            args.into_iter().chain([from.as_os_str(), path.as_os_str()]),
            None,
        );
        path
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // As `Sandbox.cleanup`: a read-only directory inside would stop the removal that follows.
        let _ = Command::new("chmod")
            .arg("-R")
            .arg("u+w")
            .arg(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Runs `git ARGS` in `cwd` (the test's own directory when `None`) with the test's environment and
/// returns its standard output, as `support.git` does; a failure panics with git's standard error.
pub fn git<I, S>(args: I, cwd: Option<&Path>) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command.args(args).stdin(Stdio::null());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let output = command.output().expect("git starts");
    assert!(
        output.status.success(),
        "{command:?} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git's output is UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bead_under_test_wins_over_the_default() {
        let chosen = select_bead(Some(OsStr::new("/opt/x/bead")), Path::new("/target/bead"));
        assert_eq!(chosen, Path::new("/opt/x/bead"));
    }

    #[test]
    fn an_unset_or_empty_bead_under_test_leaves_the_default() {
        for unset in [None, Some(OsStr::new(""))] {
            assert_eq!(
                select_bead(unset, Path::new("/target/bead")),
                Path::new("/target/bead")
            );
        }
    }

    #[test]
    #[should_panic(expected = "BEAD_UNDER_TEST must be an absolute path")]
    fn a_relative_bead_under_test_is_refused() {
        select_bead(Some(OsStr::new("target/debug/bead")), Path::new("/b"));
    }

    #[test]
    #[should_panic(expected = "the default bead must be an absolute path")]
    fn a_relative_default_is_refused() {
        select_bead(None, Path::new("bead"));
    }

    #[test]
    fn bead_mixed_with_names_the_second_bead_unless_unset_or_empty() {
        let second = select_mixed(Some(OsStr::new("/opt/y/bead")));
        assert_eq!(second.as_deref(), Some(Path::new("/opt/y/bead")));
        for unset in [None, Some(OsStr::new(""))] {
            assert_eq!(select_mixed(unset), None);
        }
    }

    #[test]
    #[should_panic(expected = "BEAD_MIXED_WITH must be an absolute path")]
    fn a_relative_bead_mixed_with_is_refused() {
        select_mixed(Some(OsStr::new("bead")));
    }
}
