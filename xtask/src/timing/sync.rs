//! `timing --sync N`: how long a publishing `bead sync` takes on a store of N events, against a
//! local bare remote. The store holds the history `timing --replay` generates as a store holds it:
//! each machine's journal in segments rotated as bead rotates them (at `--rotate BYTES`, bead's
//! default otherwise), and its attempt files. It is published once, then synced after each of a few
//! local writes: with the store's memo of its journal files in place (`seconds`), and with the memo
//! removed before each sync (`cold_seconds`), so that every file is hashed and read as bead did
//! before roll-lqjj.1. Evidence, never a gate.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use super::history::{Files, Shape, history};
use crate::workspace::{self, Failure, json_string};

/// How many syncs each figure is the best of.
const RUNS: usize = 5;

/// The rotation size bead uses when `BEAD_ROTATE_BYTES` is unset (`beadroll_store::ROTATE_BYTES`).
pub(crate) const ROTATE_BYTES: usize = 256 * 1024;

/// The workspace the store is opened as.
const WORKSPACE: &str = "timing";

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(
    count: usize,
    shape: Shape,
    rotate: usize,
    label: Option<&str>,
) -> Result<String, Failure> {
    let bead = bead()?;
    let dir = workspace::output_dir("timing")?.join(format!("sync-{}-{count}", shape.name()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    }
    let files = history(count, shape);
    let world = World::new(&dir, bead, rotate)?;
    let layout = write_store(&world.store(), &files, rotate)?;
    drop(files);
    let first = world.timed_sync()?;
    let mut warm = f64::INFINITY;
    let mut cold = f64::INFINITY;
    for run in 0..RUNS {
        world.write(&format!("warm {run}"))?;
        warm = warm.min(world.timed_sync()?);
    }
    let memo = world.store().join("sync.git").join("journal-memo");
    for run in 0..RUNS {
        world.write(&format!("cold {run}"))?;
        match std::fs::remove_file(&memo) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Failure::io("cannot remove", &memo, &error)),
        }
        cold = cold.min(world.timed_sync()?);
    }
    // An ownership-heavy store is a hundred thousand files: nothing to leave lying about.
    std::fs::remove_dir_all(&dir).map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let label = label.map_or_else(
        || match shape {
            Shape::Plain => format!("sync of {count} events"),
            Shape::Ownership => format!("sync of {count} ownership-heavy events"),
        },
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"shape\": {}, \"events\": {count}, \"rotate_bytes\": {rotate}, \
         \"segments\": {}, \"attempt_files\": {}, \"journal_bytes\": {}, \"profile\": {}, \
         \"runs\": {RUNS}, \"first_seconds\": {first:.4}, \"seconds\": {warm:.4}, \
         \"cold_seconds\": {cold:.4}, ",
        json_string(&label),
        json_string(shape.name()),
        layout.segments,
        layout.attempts,
        layout.bytes,
        json_string(profile),
    ))
}

/// The `bead` measured: `BEAD_UNDER_TEST`, else the one Cargo builds in xtask's own profile.
fn bead() -> Result<PathBuf, Failure> {
    match std::env::var_os("BEAD_UNDER_TEST").filter(|bead| !bead.is_empty()) {
        Some(bead) => std::path::absolute(&bead)
            .map_err(|error| Failure::io("cannot resolve", Path::new(&bead), &error)),
        None => crate::check::build(&workspace::root()),
    }
}

/// What the store was given.
struct Layout {
    segments: usize,
    attempts: usize,
    bytes: usize,
}

/// Writes `files` into `store` as bead holds them: each machine's journal cut into segments, a new
/// one started once the last has reached `rotate` bytes, and its attempt files.
fn write_store(store: &Path, files: &[Files], rotate: usize) -> Result<Layout, Failure> {
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
        let mut segments: Vec<Vec<u8>> = Vec::new();
        for line in machine.segment.split_inclusive(|b| *b == b'\n') {
            match segments.last_mut() {
                Some(last) if last.len() < rotate => last.extend_from_slice(line),
                _ => segments.push(line.to_vec()),
            }
        }
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
struct World {
    bead: PathBuf,
    checkout: PathBuf,
    home: PathBuf,
    rotate: usize,
}

impl World {
    fn new(dir: &Path, bead: PathBuf, rotate: usize) -> Result<World, Failure> {
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
    fn store(&self) -> PathBuf {
        self.home.join(WORKSPACE)
    }

    /// `bead ARGS` in the checkout, on the scratch store, nothing of this shell's bead or git
    /// settings inherited; a failure fails the timing.
    fn bead(&self, args: &[&str]) -> Result<(), Failure> {
        let mut command = Command::new(&self.bead);
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("BEAD_") || name.starts_with("GIT_") {
                command.env_remove(&key);
            }
        }
        let output = command
            .args(args)
            .current_dir(&self.checkout)
            .env("BEAD_HOME", &self.home)
            .env("BEAD_WORKSPACE", WORKSPACE)
            .env("BEAD_ROTATE_BYTES", self.rotate.to_string())
            .stdin(Stdio::null())
            .output()
            .map_err(|error| Failure::Failed(format!("cannot run bead: {error}")))?;
        if !output.status.success() {
            return Err(Failure::Failed(format!(
                "bead {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    /// A local write, not published: a comment on the history's first issue.
    fn write(&self, text: &str) -> Result<(), Failure> {
        self.bead(&["comments", "add", "ex-1", text, "--offline"])
    }

    /// How long a `bead sync` took, in seconds.
    fn timed_sync(&self) -> Result<f64, Failure> {
        let clock = Instant::now();
        self.bead(&["sync"])?;
        Ok(clock.elapsed().as_secs_f64())
    }
}

/// `git ARGS` in `dir`; a failure fails the timing.
fn git(dir: &Path, args: &[&str]) -> Result<(), Failure> {
    let output = Command::new("git")
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
