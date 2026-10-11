//! The sandbox: a temporary root with a local bare remote, and the `bead` its machines run.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// The variable that names the `bead` under test, as in `tests/support.py`.
pub const UNDER_TEST: &str = "BEAD_UNDER_TEST";

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

/// The `bead` Cargo builds for the workspace of the running test, found beside that test's own
/// executable: a test binary runs from `<target>/<profile>/deps/`, and the workspace's binaries
/// land in `<target>/<profile>/`, whatever the target directory, the build target or the profile.
/// It is the default `bead` of the tests of a crate other than `beadroll`, to which Cargo gives no
/// `CARGO_BIN_EXE_bead`. A build that includes the `beadroll` package makes it, as `cargo nextest
/// run --workspace` does; when it is missing, [`Sandbox::new`] fails the test and says how to build
/// it.
pub fn built_bead() -> PathBuf {
    let test = std::env::current_exe().expect("the running test's own executable");
    let profile = test
        .parent()
        .and_then(Path::parent)
        .expect("a test executable in <target>/<profile>/deps");
    profile.join("bead")
}

/// One test's world, as `support.Sandbox` builds it: a temporary root holding `origin.git`, a bare
/// remote whose `main` has one commit. Dropping it removes the root, after making everything in it
/// writable again, since tests make stores read-only.
pub struct Sandbox {
    root: PathBuf,
    origin: PathBuf,
    bead: PathBuf,
    // Removes the root when the sandbox drops, after `Drop for Sandbox` has made it writable.
    _dir: TempDir,
}

impl Sandbox {
    /// A new sandbox whose machines run `default_bead`, unless `BEAD_UNDER_TEST` names another or
    /// a machine has its own ([`crate::Machine::with_bead`]). The `bead` chosen must exist: one
    /// that does not fails the test here, saying how to build it, rather than in every command the
    /// test runs.
    pub fn new(default_bead: impl AsRef<Path>) -> Sandbox {
        let under_test = std::env::var_os(UNDER_TEST);
        Sandbox::selecting(default_bead, under_test.as_deref())
    }

    /// A new sandbox as [`Sandbox::new`] makes it, with the value of `BEAD_UNDER_TEST` given
    /// instead of read from the environment, which a test cannot change for itself alone: for the
    /// tests of the selection.
    pub fn selecting(default_bead: impl AsRef<Path>, under_test: Option<&OsStr>) -> Sandbox {
        let bead = select_bead(under_test, default_bead.as_ref());
        assert!(
            bead.is_file(),
            "no bead at {}: build it first (`cargo build --package beadroll`, which `cargo nextest \
             run --workspace` does too), or name one in {UNDER_TEST} by its absolute path",
            bead.display()
        );
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

    /// The `bead` this sandbox's machines run unless they have their own.
    pub fn bead(&self) -> &Path {
        &self.bead
    }

    /// A new bare repository `name` in the sandbox, whose default branch is `main`, and which never
    /// maintains itself: after every push git would start its auto maintenance there
    /// (`receive.autogc`), detached, and repack the remote behind the test's back, packing the very
    /// objects a test inspects or damages (roll-k2wy.20). It changes only when a test or bead
    /// changes it.
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
        let config: [&OsStr; 4] = [
            "--git-dir".as_ref(),
            path.as_os_str(),
            "config".as_ref(),
            "receive.autogc".as_ref(),
        ];
        git(config.into_iter().chain(["false".as_ref()]), None);
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
    #[should_panic(expected = "no bead at /nonexistent/bead: build it first")]
    fn a_bead_that_does_not_exist_fails_the_test_saying_how_to_build_it() {
        let _ = Sandbox::selecting(Path::new("/nonexistent/bead"), None);
    }

    #[test]
    fn the_built_bead_is_in_the_profile_directory_above_the_tests() {
        let test = std::env::current_exe().unwrap();
        let profile = test.parent().and_then(Path::parent).unwrap();
        assert_eq!(built_bead(), profile.join("bead"));
        assert_eq!(test.parent().and_then(Path::file_name).unwrap(), "deps");
    }

    /// What a test pushes into a sandbox remote stays as it arrived (roll-k2wy.20). git's auto
    /// maintenance after a push counts loose objects by `objects/17` alone, times 256, so two loose
    /// objects there read as 512, over the 100 at which git 2.55's geometric repack runs; run in the
    /// foreground, as here, a remote that maintained itself would answer the push with every object
    /// packed and none left loose.
    #[test]
    fn a_push_leaves_the_remotes_objects_as_they_arrived() {
        let sandbox = Sandbox::selecting(std::env::current_exe().unwrap(), None);
        let origin = sandbox.origin();
        let remote = |args: &[&str]| {
            let git_dir: [&OsStr; 2] = ["--git-dir".as_ref(), origin.as_os_str()];
            git(git_dir.into_iter().chain(args.iter().map(OsStr::new)), None)
        };
        // two loose objects in objects/17: candidates hashed in the remote's own format, two of them
        // written
        let candidates = sandbox.root().join("candidates");
        std::fs::create_dir(&candidates).unwrap();
        let paths: Vec<String> = (0..2000)
            .map(|n| {
                let path = candidates.join(n.to_string());
                std::fs::write(&path, format!("loose object {n}\n")).unwrap();
                path.display().to_string()
            })
            .collect();
        let mut hash = vec!["hash-object"];
        hash.extend(paths.iter().map(String::as_str));
        let ids = remote(&hash);
        let named_17: Vec<&str> = ids
            .lines()
            .zip(&paths)
            .filter(|(id, _)| id.starts_with("17"))
            .map(|(_, path)| path.as_str())
            .take(2)
            .collect();
        let mut write = vec!["hash-object", "-w"];
        write.extend(&named_17);
        remote(&write);
        // at least two: the sandbox's own seed commit, named by the time it was made, may be one more
        let seventeen = std::fs::read_dir(origin.join("objects").join("17")).unwrap();
        assert!(seventeen.count() >= 2, "{named_17:?}");
        // a remote that maintained itself would do it before the push returns
        remote(&["config", "maintenance.autoDetach", "false"]);
        remote(&["config", "gc.autoDetach", "false"]);
        let work = sandbox.clone("pusher", None);
        let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
        let commit = ["commit", "-q", "--allow-empty", "-m", "pushed"];
        git(identity.into_iter().chain(commit), Some(&work));
        git(["push", "-q", "origin", "HEAD:main"], Some(&work));
        let pushed = git(["rev-parse", "HEAD"], Some(&work));
        let pushed = pushed.trim();
        let loose = origin.join("objects").join(&pushed[..2]).join(&pushed[2..]);
        let packs: Vec<_> = std::fs::read_dir(origin.join("objects").join("pack"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(loose.exists(), "the pushed commit was packed: {packs:?}");
        assert!(packs.is_empty(), "the remote repacked itself: {packs:?}");
    }
}
