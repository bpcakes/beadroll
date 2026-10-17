//! What `timing --sync`, `--read` and `--merge` run `bead` on: a scratch checkout whose origin is a
//! local bare repository, a scratch `BEAD_HOME`, and in it the store of a history `timing --replay`
//! generates, as a store holds it.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use super::history::{Files, segments};
use super::memory::{self, Peaks};
use crate::workspace::{self, Failure};

/// How many runs each figure is the best of.
pub(super) const RUNS: usize = 5;

/// The profile xtask was built in, which the `bead` it builds shares.
pub(super) fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// The workspace the store is opened as.
const WORKSPACE: &str = "timing";

/// The `bead` measured: `BEAD_UNDER_TEST`, else the one Cargo builds in xtask's own profile.
pub(super) fn bead() -> Result<PathBuf, Failure> {
    match std::env::var_os("BEAD_UNDER_TEST").filter(|bead| !bead.is_empty()) {
        Some(bead) => std::path::absolute(&bead)
            .map_err(|error| Failure::io("cannot resolve", Path::new(&bead), &error)),
        None => crate::check::build(&workspace::root()),
    }
}

/// What the store was given.
pub(super) struct Layout {
    pub(super) segments: usize,
    pub(super) attempts: usize,
    pub(super) bytes: usize,
}

/// Writes `files` into `store` as bead holds them: each machine's journal cut into segments, a new
/// one started once the last has reached `rotate` bytes, and its attempt files.
pub(super) fn write_store(store: &Path, files: &[Files], rotate: usize) -> Result<Layout, Failure> {
    let mut layout = Layout {
        segments: 0,
        attempts: 0,
        bytes: 0,
    };
    for machine in files {
        let folder = store.join("events").join(&machine.replica);
        let claims = folder.join("claims");
        std::fs::create_dir_all(&claims)
            .map_err(|error| Failure::io("cannot create", &claims, &error))?;
        let segments = segments(&machine.segment, rotate);
        for (n, data) in segments.iter().enumerate() {
            let path = folder.join(format!("{:06}.jsonl", n + 1));
            std::fs::write(&path, data)
                .map_err(|error| Failure::io("cannot write", &path, &error))?;
            layout.bytes += data.len();
        }
        layout.segments += segments.len();
        for (name, data) in &machine.attempts {
            let path = claims.join(name);
            std::fs::write(&path, data)
                .map_err(|error| Failure::io("cannot write", &path, &error))?;
            layout.bytes += data.len();
        }
        layout.attempts += machine.attempts.len();
    }
    Ok(layout)
}

/// A checkout whose origin is a local bare repository, and a scratch `BEAD_HOME`.
pub(super) struct World {
    bead: PathBuf,
    checkout: PathBuf,
    home: PathBuf,
    rotate: usize,
}

impl World {
    pub(super) fn new(dir: &Path, bead: PathBuf, rotate: usize) -> Result<World, Failure> {
        let origin = dir.join("origin.git");
        let checkout = dir.join("work");
        let home = dir.join("home");
        std::fs::create_dir_all(&home)
            .map_err(|error| Failure::io("cannot create", &home, &error))?;
        git(dir, &["init", "--quiet", "--bare", "origin.git"])?;
        git(dir, &["init", "--quiet", "work"])?;
        let origin = origin.to_string_lossy().into_owned();
        git(&checkout, &["remote", "add", "origin", &origin])?;
        let identity = [
            "-c",
            "user.name=timing",
            "-c",
            "user.email=timing@example.invalid",
        ];
        let commit = ["commit", "--quiet", "--allow-empty", "-m", "timing"];
        git(&checkout, &[&identity[..], &commit[..]].concat())?;
        Ok(World {
            bead,
            checkout,
            home,
            rotate,
        })
    }

    /// The store, as `BEAD_WORKSPACE` names it.
    pub(super) fn store(&self) -> PathBuf {
        self.home.join(WORKSPACE)
    }

    /// How many bytes the store's file `name` holds (`projection`, `projection-texts`).
    pub(super) fn stored_bytes(&self, name: &str) -> Result<u64, Failure> {
        let path = self.store().join(name);
        let metadata =
            std::fs::metadata(&path).map_err(|error| Failure::io("cannot read", &path, &error))?;
        Ok(metadata.len())
    }

    /// `bead ARGS` in the checkout, on the scratch store ([`isolated`]); a failure fails the
    /// timing.
    pub(super) fn bead(&self, args: &[&str]) -> Result<(), Failure> {
        self.output(args, None).map(drop)
    }

    /// What `bead ARGS` printed, run as [`World::bead`] runs it, with `failpoints` as its
    /// `BEAD_FAILPOINTS` when given.
    pub(super) fn output(
        &self,
        args: &[&str],
        failpoints: Option<&str>,
    ) -> Result<String, Failure> {
        let mut command = isolated(&self.bead);
        command
            .args(args)
            .current_dir(&self.checkout)
            .env("BEAD_HOME", &self.home)
            .env("BEAD_WORKSPACE", WORKSPACE)
            .env("BEAD_ROTATE_BYTES", self.rotate.to_string())
            .stdin(Stdio::null());
        if let Some(failpoints) = failpoints {
            command.env("BEAD_FAILPOINTS", failpoints);
        }
        let output = command
            .output()
            .map_err(|error| Failure::Failed(format!("cannot run bead: {error}")))?;
        if !output.status.success() {
            return Err(Failure::Failed(format!(
                "bead {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// How long `bead ARGS` took, in seconds, and what it held at its peak: its resident set, as
    /// `/usr/bin/time` reports it when it exits (`None` where there is no such report), and its
    /// heap when `counted` is xtask's own executable, which runs it as `cargo xtask bead` does
    /// ([`memory::bead`]).
    pub(super) fn peaks(
        &self,
        args: &[&str],
        counted: Option<&Path>,
    ) -> Result<(f64, Peaks), Failure> {
        let flag = if cfg!(target_os = "macos") {
            "-l"
        } else {
            "-v"
        };
        let program = counted.unwrap_or(&self.bead);
        let run = |timed: bool| {
            let mut command = isolated(if timed {
                Path::new("/usr/bin/time")
            } else {
                program
            });
            if timed {
                command.arg(flag).arg(program);
            }
            if counted.is_some() {
                command.arg("bead");
            }
            command
                .args(args)
                .current_dir(&self.checkout)
                .env("BEAD_HOME", &self.home)
                .env("BEAD_WORKSPACE", WORKSPACE)
                .env("BEAD_ROTATE_BYTES", self.rotate.to_string())
                .stdin(Stdio::null())
                .output()
        };
        let clock = Instant::now();
        // no /usr/bin/time here: the heap is still xtask's to say
        let (output, timed) = match run(true) {
            Ok(output) => (output, true),
            Err(_) => (
                run(false).map_err(|error| Failure::Failed(format!("cannot run bead: {error}")))?,
                false,
            ),
        };
        let seconds = clock.elapsed().as_secs_f64();
        if !output.status.success() {
            return Err(Failure::Failed(format!(
                "bead {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let report = String::from_utf8_lossy(&output.stderr);
        let peaks = Peaks {
            resident: timed.then(|| memory::peak_of(&report)).flatten(),
            heap: counted.and_then(|_| memory::heap_of(&report)),
        };
        Ok((seconds, peaks))
    }

    /// A local write, not published: a comment on the history's first issue.
    pub(super) fn write(&self, text: &str) -> Result<(), Failure> {
        self.bead(&["comments", "add", "ex-1", text, "--offline"])
    }

    /// Takes the checkout's origin away, the remote its store publishes to, so that the store has
    /// none: a write that is not `--offline` then publishes nowhere (roll-lqjj.50).
    pub(super) fn without_remote(&self) -> Result<(), Failure> {
        git(&self.checkout, &["remote", "remove", "origin"])
    }

    /// How long `bead ARGS` took, in seconds.
    pub(super) fn timed(&self, args: &[&str]) -> Result<f64, Failure> {
        let clock = Instant::now();
        self.bead(args)?;
        Ok(clock.elapsed().as_secs_f64())
    }

    /// Removes the store's file `name` (`projection`, `sync.git/journal-memo`), if it is there.
    pub(super) fn forget(&self, name: &str) -> Result<(), Failure> {
        let path = self.store().join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Failure::io("cannot remove", &path, &error)),
        }
    }

    /// Another machine on the same remote, under `dir`: a checkout cloned from it and a
    /// `BEAD_HOME` of its own, where it opens the same workspace.
    pub(super) fn another(&self, dir: &Path) -> Result<World, Failure> {
        let origin = self.checkout.with_file_name("origin.git");
        let checkout = dir.join("work");
        let home = dir.join("home");
        std::fs::create_dir_all(&home)
            .map_err(|error| Failure::io("cannot create", &home, &error))?;
        let origin = origin.to_string_lossy().into_owned();
        git(dir, &["clone", "--quiet", &origin, "work"])?;
        Ok(World {
            bead: self.bead.clone(),
            checkout,
            home,
            rotate: self.rotate,
        })
    }
}

/// `program`, run with none of this shell's `BEAD_*` and `GIT_*` variables: they would take bead
/// to another store, or git to another repository (`GIT_DIR`), than the scratch ones.
pub(super) fn isolated(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("BEAD_") || name.starts_with("GIT_") {
            command.env_remove(&key);
        }
    }
    command
}

/// `git ARGS` in `dir`, in the scratch repositories alone ([`isolated`]); a failure fails the
/// timing.
pub(super) fn git(dir: &Path, args: &[&str]) -> Result<(), Failure> {
    let output = isolated("git")
        .args(args.iter().map(OsStr::new))
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| Failure::Failed(format!("cannot run git: {error}")))?;
    if !output.status.success() {
        return Err(Failure::Failed(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::history::{Shape, history};

    #[test]
    fn the_scratch_gits_and_beads_inherit_no_repository_or_store() {
        let command = isolated("git");
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy().into_owned();
            let ours = name.starts_with("BEAD_") || name.starts_with("GIT_");
            assert_eq!(removed.contains(&name), ours, "{name}");
        }
    }

    #[test]
    fn a_journal_is_cut_where_bead_would_rotate_it() {
        let dir = tempfile::tempdir().unwrap();
        let files = history(400, Shape::Ownership);
        let layout = write_store(dir.path(), &files, 4096).unwrap();
        let total: usize = files
            .iter()
            .map(|m| m.segment.len() + m.attempts.iter().map(|(_, d)| d.len()).sum::<usize>())
            .sum();
        assert_eq!(layout.bytes, total);
        assert!(
            layout.segments > files.len(),
            "{} segments",
            layout.segments
        );
        for machine in &files {
            let folder = dir.path().join("events").join(&machine.replica);
            let mut segments: Vec<PathBuf> = std::fs::read_dir(&folder)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
                .collect();
            segments.sort();
            let mut joined = Vec::new();
            for (n, path) in segments.iter().enumerate() {
                let data = std::fs::read(path).unwrap();
                assert!(data.ends_with(b"\n"), "whole lines only");
                if n + 1 < segments.len() {
                    // full: the line that reached the size is the last one written to it
                    assert!(data.len() >= 4096);
                    let before_last = data[..data.len() - 1].iter().rposition(|b| *b == b'\n');
                    assert!(before_last.map_or(0, |at| at + 1) < 4096);
                }
                joined.extend(data);
            }
            assert_eq!(joined, machine.segment);
        }
    }
}
