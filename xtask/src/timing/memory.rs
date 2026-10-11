//! The memory replay needs: the peak resident set of a process that does nothing but load a journal
//! written to disk and replay it (`timing --load DIR`), so that neither the generator nor xtask's
//! own copies count. The kernel keeps every process's peak; `/usr/bin/time` reports it when the
//! process exits (`-l` on macOS, `-v` with GNU time on Linux).

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use beadroll_core::journal::{Journal, read_attempt, read_segment};
use beadroll_core::replay::replay;

use super::history::Files;
use crate::workspace::Failure;

/// Writes `files` under `dir` as a store holds them: `REPLICA/000001.jsonl` and
/// `REPLICA/claims/ID.json`, replacing whatever was there.
pub(crate) fn write_files(dir: &Path, files: &[Files]) -> Result<(), Failure> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|error| Failure::io("cannot remove", dir, &error))?;
    }
    for machine in files {
        let claims = dir.join(&machine.replica).join("claims");
        std::fs::create_dir_all(&claims)
            .map_err(|error| Failure::io("cannot create", &claims, &error))?;
        let segment = dir.join(&machine.replica).join("000001.jsonl");
        std::fs::write(&segment, &machine.segment)
            .map_err(|error| Failure::io("cannot write", &segment, &error))?;
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
                for event in read_segment(&data, &origin, Some(&mut journal.invalid)) {
                    journal.add(event, &replica, &origin, true);
                }
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
            for event in read_attempt(&data, &origin, Some(&mut journal.invalid)) {
                journal.add(event, &replica, &origin, false);
            }
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
/// `--no-replay` only loads it, and says how many events and issues there were.
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
    Ok(ExitCode::SUCCESS)
}

/// The peak resident set of `program ARGS` in bytes, as `/usr/bin/time` reports it when the
/// program exits; `None` where there is no such report.
pub(crate) fn peak(program: &Path, args: &[&std::ffi::OsStr]) -> Result<Option<u64>, Failure> {
    let flag = if cfg!(target_os = "macos") {
        "-l"
    } else {
        "-v"
    };
    let output = Command::new("/usr/bin/time")
        .arg(flag)
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .output();
    let Ok(output) = output else {
        return Ok(None); // no /usr/bin/time here
    };
    if !output.status.success() {
        return Err(Failure::Failed(format!(
            "{} failed: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(peak_of(&String::from_utf8_lossy(&output.stderr)))
}

/// The peak in `/usr/bin/time`'s report: macOS prints bytes, GNU time kilobytes.
fn peak_of(report: &str) -> Option<u64> {
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
