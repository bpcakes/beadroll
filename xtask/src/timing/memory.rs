//! The memory replay needs: the peak resident set of a process that does nothing but load a journal
//! written to disk and replay it (`timing --load DIR`), so that neither the generator nor xtask's
//! own copies count. The kernel keeps every process's peak; `/usr/bin/time` reports it when the
//! process exits (`-l` on macOS, `-v` with GNU time on Linux). `cargo xtask bead` is such a
//! process for a bead command: bead's own code, run in xtask, whose heap is counted.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use beadroll_core::journal::Journal;
use beadroll_core::replay::replay;

use super::heap;
use super::history::{Files, ROTATE_BYTES, segments};
use crate::workspace::Failure;

/// Writes `files` under `dir` as a store holds them: `REPLICA/000001.jsonl` on, rotated as bead
/// rotates its journal, and `REPLICA/claims/ID.json`, replacing whatever was there. The process
/// that loads them then reads a segment at a time, as bead does.
pub(crate) fn write_files(dir: &Path, files: &[Files]) -> Result<(), Failure> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|error| Failure::io("cannot remove", dir, &error))?;
    }
    for machine in files {
        let claims = dir.join(&machine.replica).join("claims");
        std::fs::create_dir_all(&claims)
            .map_err(|error| Failure::io("cannot create", &claims, &error))?;
        for (n, data) in segments(&machine.segment, ROTATE_BYTES).iter().enumerate() {
            let segment = dir
                .join(&machine.replica)
                .join(format!("{:06}.jsonl", n + 1));
            std::fs::write(&segment, data)
                .map_err(|error| Failure::io("cannot write", &segment, &error))?;
        }
        for (name, data) in &machine.attempts {
            let path = claims.join(name);
            std::fs::write(&path, data)
                .map_err(|error| Failure::io("cannot write", &path, &error))?;
        }
    }
    Ok(())
}

/// The journal in `dir`, read as a store's `load_journal` reads one: every machine's segments,
/// then every machine's attempt files, in name order, settled.
pub(crate) fn load(dir: &Path) -> Result<Journal, Failure> {
    let mut journal = Journal::new();
    let machines = sorted(dir)?;
    for machine in &machines {
        let replica = machine.file_name().unwrap_or_default().to_string_lossy();
        for path in sorted(machine)? {
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                let data = std::fs::read(&path)
                    .map_err(|error| Failure::io("cannot read", &path, &error))?;
                let origin = path.display().to_string();
                journal.add_segment(&data, &replica, &origin);
            }
        }
    }
    for machine in &machines {
        let replica = machine.file_name().unwrap_or_default().to_string_lossy();
        let claims = machine.join("claims");
        for path in sorted(&claims)? {
            let data =
                std::fs::read(&path).map_err(|error| Failure::io("cannot read", &path, &error))?;
            let origin = path.display().to_string();
            journal.add_attempt(&data, &replica, &origin);
        }
    }
    journal.settle();
    Ok(journal)
}

/// A directory's entries, sorted; none when it does not exist.
fn sorted(dir: &Path) -> Result<Vec<std::path::PathBuf>, Failure> {
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Failure::io("cannot list", dir, &error)),
    };
    let mut paths = Vec::new();
    for entry in listing {
        paths.push(
            entry
                .map_err(|error| Failure::io("cannot list", dir, &error))?
                .path(),
        );
    }
    paths.sort();
    Ok(paths)
}

/// `timing --load DIR [--no-replay]`: loads the journal in `dir` and replays it once, or with
/// `--no-replay` only loads it, and says how many events and issues there were, and the most the
/// process held on its heap at once ([`heap::peak`]).
pub(crate) fn main(dir: &Path, replaying: bool) -> Result<ExitCode, Failure> {
    let journal = load(dir)?;
    let events = journal.events().len();
    if replaying {
        let state =
            replay(journal).map_err(|error| Failure::Failed(format!("replay failed: {error}")))?;
        println!("{events} events, {} issues", state.issues().count());
    } else {
        println!("{events} events");
    }
    println!("{HEAP_PEAK} {}", heap::peak());
    Ok(ExitCode::SUCCESS)
}

/// `cargo xtask bead ARG...`: bead run with ARGs in this process, its exit status passed on, and
/// once it is done the most the process held on its heap at once ([`heap::peak`]) said on standard
/// error, after whatever bead wrote there (roll-lqjj.9), and what it still held ([`heap::held`]):
/// `run_with` frees what the command made, so that is a few kilobytes of the process's own, and a
/// tracker's projection would be megabytes (roll-lqjj.42).
pub(crate) fn bead(args: Vec<OsString>) -> ExitCode {
    let code = beadroll::run_with(args);
    eprintln!("{HEAP_PEAK} {}", heap::peak());
    eprintln!("{HEAP_HELD} {}", heap::held());
    code
}

/// How `--load` and `cargo xtask bead` say their heap peak.
const HEAP_PEAK: &str = "heap peak:";

/// How `cargo xtask bead` says what its process still held when bead returned.
const HEAP_HELD: &str = "heap held:";

/// What a `--load` run held at its peak, in bytes: its resident set, as `/usr/bin/time` reports
/// it, and its heap, as it reported it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Peaks {
    pub(crate) resident: Option<u64>,
    pub(crate) heap: Option<u64>,
}

/// The peaks of `program ARGS`, a `--load` run: its resident set, as `/usr/bin/time` reports it
/// when the program exits (`None` where there is no such report), and its heap, as the program
/// says.
pub(crate) fn peaks(program: &Path, args: &[&std::ffi::OsStr]) -> Result<Peaks, Failure> {
    let flag = if cfg!(target_os = "macos") {
        "-l"
    } else {
        "-v"
    };
    let timed = Command::new("/usr/bin/time")
        .arg(flag)
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .output();
    let (output, timed) = match timed {
        Ok(output) => (output, true),
        // no /usr/bin/time here: the heap is still the program's to say
        Err(_) => {
            let output = Command::new(program)
                .args(args)
                .stdin(Stdio::null())
                .output()
                .map_err(|error| Failure::io("cannot run", program, &error))?;
            (output, false)
        }
    };
    if !output.status.success() {
        return Err(Failure::Failed(format!(
            "{} failed: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(Peaks {
        resident: timed
            .then(|| peak_of(&String::from_utf8_lossy(&output.stderr)))
            .flatten(),
        heap: heap_of(&String::from_utf8_lossy(&output.stdout)),
    })
}

/// The heap peak a `--load` or `cargo xtask bead` run said it reached.
pub(super) fn heap_of(said: &str) -> Option<u64> {
    said.lines()
        .find_map(|line| line.strip_prefix(HEAP_PEAK)?.trim().parse().ok())
}

/// The peak in `/usr/bin/time`'s report: macOS prints bytes, GNU time kilobytes.
pub(super) fn peak_of(report: &str) -> Option<u64> {
    report.lines().find_map(|line| {
        let line = line.trim();
        if let Some(bytes) = line.strip_suffix("maximum resident set size") {
            return bytes.trim().parse().ok();
        }
        let kilobytes = line.strip_prefix("Maximum resident set size (kbytes):")?;
        kilobytes.trim().parse::<u64>().ok().map(|k| k * 1024)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timing::history::{Shape, history};

    #[test]
    fn both_reports_are_read() {
        let macos = "        0.01 real         0.00 user         0.00 sys\n\
                     \x20            1490944  maximum resident set size\n\
                     \x20                  0  average shared memory size\n";
        assert_eq!(peak_of(macos), Some(1_490_944));
        let gnu = "\tCommand being timed: \"true\"\n\tMaximum resident set size (kbytes): 1876\n";
        assert_eq!(peak_of(gnu), Some(1876 * 1024));
        assert_eq!(peak_of("sh: /usr/bin/time: not found"), None);
        assert_eq!(heap_of("200 events\nheap peak: 4096\n"), Some(4096));
        assert_eq!(heap_of("200 events\n"), None);
    }

    #[test]
    fn a_history_written_to_disk_loads_as_it_was_generated() {
        let dir = tempfile::tempdir().unwrap();
        let files = history(500, Shape::Ownership);
        write_files(dir.path(), &files).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.events().len(), 500);
        assert!(loaded.invalid.is_empty());
        let sequenced: usize = loaded.order().values().map(Vec::len).sum();
        let attempted: usize = files
            .iter()
            .flat_map(|machine| &machine.attempts)
            .map(|(_, data)| data.iter().filter(|b| **b == b'\n').count())
            .sum();
        assert_eq!(sequenced + attempted, 500);
        assert!(attempted > 0);
    }
}
