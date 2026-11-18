//! The sandbox: a temporary root with a local bare remote, and the `bead` its machines run.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// The environment variable selecting the binary under test.
pub const UNDER_TEST: &str = "BEAD_UNDER_TEST";

/// Selects a nonempty under_test value, otherwise default. Both must be absolute paths because
/// commands run from multiple directories; relative paths panic during setup.
fn select_bead(under_test: Option<&OsStr>, default: &Path) -> PathBuf {
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
/// executable: Cargo puts tests in `<target>/<profile>/build/<package>/<hash>/out/` (or `deps/`
/// with older Cargo), and binaries in `<target>/<profile>/`, whatever the target directory,
/// the build target or the profile.
/// It is the default `bead` of the tests of a crate other than `beadroll`, to which Cargo gives no
/// `CARGO_BIN_EXE_bead`. A build that includes the `beadroll` package makes it, as `cargo nextest
/// run --workspace` does; when it is missing, [`Sandbox::new`] fails the test and says how to build
/// it.
pub fn built_bead() -> PathBuf {
    let test = std::env::current_exe().expect("the running test's own executable");
    bead_for_test(&test)
}

/// Recognizes Cargo's test layouts without depending on a target directory or profile name.
fn bead_for_test(test: &Path) -> PathBuf {
    let output = test.parent().expect("a test executable's directory");
    let profile = match output.file_name().and_then(OsStr::to_str) {
        Some("deps") => output.parent(),
        Some("out") => output
            .ancestors()
            .nth(3)
            .filter(|dir| dir.file_name() == Some(OsStr::new("build")))
            .and_then(Path::parent),
        _ => None,
    }
    .expect("a test executable in <profile>/build/<package>/<hash>/out or <profile>/deps");
    profile.join("bead")
}

/// A temporary sandbox with a bare origin.git whose main has one commit. Dropping restores write
/// permissions and removes the root, including stores made read-only by tests.
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
        // Resolve symlinks so paths match bead's output; macOS temporary directories are reached
        // through a symlink.
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

    /// Writes `rows` as JSON lines ([`crate::json_lines`]) to the file `name` at the sandbox's root, a
    /// Beads export for a test to import: its path.
    pub fn records(&self, name: &str, rows: &[crate::Json]) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, crate::json_lines(rows))
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        path
    }

    /// What `git --git-dir ORIGIN ARGS` printed, run on the bare remote as [`git`] runs git: a
    /// failure panics with git's standard error.
    pub fn origin_git<I, S>(&self, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let at = [OsStr::new("--git-dir"), self.origin.as_os_str()];
        git(
            at.into_iter()
                .map(OsStr::to_owned)
                .chain(args.into_iter().map(|a| a.as_ref().to_owned())),
            None,
        )
    }

    /// An ssh stand-in for `GIT_SSH_COMMAND`, `fake-ssh` at the sandbox's root: it answers `-G`
    /// (the configuration ssh would use) with nothing, and runs the remote command locally, with
    /// git's own programs on `PATH`, in the directory `$FAKE_SSH_ROOT` names. Its path.
    pub fn fake_ssh(&self) -> PathBuf {
        let path = self.root.join("fake-ssh");
        let script = concat!(
            "#!/bin/sh\n",
            "[ \"$1\" = \"-G\" ] && exit 0\n",
            "for last; do :; done\n",
            "PATH=\"$(git --exec-path):$PATH\"\n",
            "cd \"$FAKE_SSH_ROOT\" && exec sh -c \"$last\"\n",
        );
        crate::executable(&path, script);
        path
    }

    /// What `git ARGS` printed, stripped, run as plumbing on the bare remote (`GIT_DIR`, from inside
    /// it) as author and committer `t`, `input` on its standard input: objects and refs a test
    /// writes there by hand. A failure panics with git's standard error.
    pub fn origin_plumbing(&self, args: &[&str], input: Option<&[u8]>) -> String {
        let mut command = git_command();
        command
            .args(args)
            .current_dir(&self.origin)
            .env("GIT_DIR", &self.origin)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("git starts");
        let pipe = child.stdin.take();
        let output = std::thread::scope(|scope| {
            // written from another thread, so that git never waits for us while we wait for it
            if let (Some(mut pipe), Some(input)) = (pipe, input) {
                scope.spawn(move || std::io::Write::write_all(&mut pipe, input));
            }
            child.wait_with_output().expect("git's output")
        });
        assert!(
            output.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        let printed = String::from_utf8(output.stdout).expect("git's output is UTF-8");
        printed.trim().to_owned()
    }

    /// The commit the remote's `refs/beads/journal` names, or `None` when it names none: no
    /// publish has reached it yet. Any other failure of git's panics, so that it is never taken
    /// for a remote without a journal.
    pub fn remote_journal(&self) -> Option<String> {
        let output = git_command()
            .arg("--git-dir")
            .arg(&self.origin)
            .args(["rev-parse", "--verify", "--quiet", "refs/beads/journal"])
            .stdin(Stdio::null())
            .output()
            .expect("git starts");
        match output.status.code() {
            Some(0) => Some(
                String::from_utf8(output.stdout)
                    .expect("a commit id")
                    .trim()
                    .to_owned(),
            ),
            // --verify --quiet: the ref does not exist, and git says nothing
            Some(1) if output.stdout.is_empty() => None,
            _ => panic!(
                "git rev-parse refs/beads/journal on the remote failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ),
        }
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

/// `git` for a fixture, with none of the `GIT_*` variables among `inherited`: one that a hook or a
/// wrapper the tests were started under exports (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`)
/// would turn a fixture's commit and push on the repository they were started from. It starts no
/// auto maintenance (`maintenance.auto`): after a commit git would start it detached, and it would
/// take `objects/maintenance.lock` in the fixture's repository whenever it got to it, a file a test
/// that compares a repository before and after (`assert_git_isolation`) then sees changed.
fn fixture_git(inherited: impl Iterator<Item = OsString>) -> Command {
    let mut command = Command::new("git");
    for name in inherited.filter(|name| name.as_encoded_bytes().starts_with(b"GIT_")) {
        command.env_remove(name);
    }
    command.args(["-c", "maintenance.auto=false"]);
    command
}

/// A fixture's git command, without inherited `GIT_*` variables. Callers may then supply their
/// own environment overrides, byte arguments, input or output handling without losing isolation.
pub fn git_command() -> Command {
    fixture_git(std::env::vars_os().map(|(name, _)| name))
}

/// Runs git in cwd, or the inherited directory when absent, with the test environment less every
/// `GIT_*` variable, and returns stdout. Failure panics with git's stderr.
pub fn git<I, S>(args: I, cwd: Option<&Path>) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = git_command();
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

    /// AS08: a fixture's git is started without the `GIT_*` variables the tests inherited, and
    /// with every other one.
    #[test]
    fn a_fixtures_git_takes_none_of_the_inherited_git_variables() {
        let inherited = ["GIT_DIR", "GIT_INDEX_FILE", "GITHUB_TOKEN", "PATH"];
        let command = fixture_git(inherited.into_iter().map(OsString::from));
        let removed: Vec<_> = (command.get_envs())
            .map(|(name, value)| (name.to_str().unwrap(), value.is_none()))
            .collect();
        assert_eq!(removed, [("GIT_DIR", true), ("GIT_INDEX_FILE", true)]);
    }

    /// A remote no publish reached has no journal; one whose `refs/beads/journal` names a commit
    /// answers that commit; a remote git cannot read at all panics, never passing for one without
    /// a journal.
    #[test]
    fn the_remote_journal_is_its_commit_or_none_and_a_broken_remote_panics() {
        let sandbox = Sandbox::selecting("/bin/sh", None);
        assert_eq!(sandbox.remote_journal(), None);
        let tree = sandbox.origin_git(["hash-object", "-t", "tree", "-w", "/dev/null"]);
        let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
        let commit =
            sandbox.origin_git(
                identity
                    .into_iter()
                    .chain(["commit-tree", tree.trim(), "-m", "j"]),
            );
        let commit = commit.trim();
        sandbox.origin_git(["update-ref", "refs/beads/journal", commit]);
        assert_eq!(sandbox.remote_journal().as_deref(), Some(commit));
        std::fs::remove_dir_all(sandbox.origin()).unwrap();
        std::fs::create_dir(sandbox.origin()).unwrap();
        let broken = std::panic::catch_unwind(|| sandbox.remote_journal());
        assert!(
            broken.is_err(),
            "a remote that is no repository answered {broken:?}"
        );
    }

    /// A fixture's commit starts no auto maintenance, which git would otherwise start detached and
    /// which would take a lock in the repository whenever it got to it: what made
    /// `assert_git_isolation` see a fixture's own repository changed, now and then, on CI.
    #[test]
    fn a_fixtures_commit_starts_no_auto_maintenance() {
        let dir = tempfile::tempdir().unwrap();
        git(["init", "-q", "-b", "main"], Some(dir.path()));
        let traced = git_command()
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(["commit", "-q", "--allow-empty", "-m", "m"])
            .current_dir(dir.path())
            .env("GIT_TRACE", "1")
            .stdin(Stdio::null())
            .output()
            .expect("git starts");
        assert!(traced.status.success(), "{traced:?}");
        let trace = String::from_utf8_lossy(&traced.stderr);
        assert!(trace.contains("commit"), "no trace: {trace}");
        assert!(!trace.contains("maintenance run"), "{trace}");
    }

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
    fn the_built_bead_is_found_in_both_cargo_test_layouts() {
        for (test, bead) in [
            ("/target/debug/deps/test-hash", "/target/debug/bead"),
            (
                "/target/debug/build/beadroll-sync/hash/out/test-hash",
                "/target/debug/bead",
            ),
            (
                "/custom/target/triple/release/deps/test-hash",
                "/custom/target/triple/release/bead",
            ),
            (
                "/custom/target/triple/release/build/beadroll-sync/hash/out/test-hash",
                "/custom/target/triple/release/bead",
            ),
        ] {
            assert_eq!(bead_for_test(Path::new(test)), Path::new(bead), "{test}");
        }
    }

    #[test]
    #[should_panic(expected = "a test executable in <profile>/build")]
    fn an_unknown_test_layout_is_refused() {
        bead_for_test(Path::new("/target/debug/unknown/package/hash/out/test"));
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
