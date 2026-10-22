//! The temporary files a cache's writers left beside it when they died (roll-lqjj.52):
//! `.NAME.PID.tmp` beside NAME, with the id of a process that is gone. A bead killed between the
//! write of one and its rename into place (a crash, a SIGKILL) leaves it, as large as the file,
//! and nothing else would ever remove it, since its name is that process's own. Each atomic
//! replacement of that cache tries to remove these copies; text-file appends do not.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use rustix::io::Errno;
use rustix::process::{Pid, test_kill_process};

/// Tries to remove the temporary files beside `path` whose process id no longer exists. A process
/// id still present, including one reused by another process, keeps its file at this check. This
/// assumes the store's writers share one host and PID namespace. A directory that cannot be listed,
/// or a file that cannot be removed, is left as it is: what a cache's writers left is no failure of
/// this one.
pub(super) fn remove(path: &Path) {
    let Some(target) = path.file_name() else {
        return;
    };
    let parent = path.parent().unwrap_or(Path::new("."));
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if let Some(pid) = writer_of(&entry.file_name(), target)
            && !running(pid)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The process whose temporary file for `target` is named `name`: `.TARGET.PID.tmp`, the id
/// written as [`super::temporary`] writes it, and nothing else (another target's, a name that is
/// not a process's, one a person made up).
fn writer_of(name: &OsStr, target: &OsStr) -> Option<Pid> {
    let digits = (name.as_bytes().strip_prefix(b".")?)
        .strip_prefix(target.as_bytes())?
        .strip_prefix(b".")?
        .strip_suffix(b".tmp")?;
    let pid: u32 = std::str::from_utf8(digits).ok()?.parse().ok()?;
    // as bead writes it: no sign, no leading zero
    if pid.to_string().as_bytes() != digits {
        return None;
    }
    Pid::from_raw(i32::try_from(pid).ok()?)
}

/// Whether a process with `pid` is running: one that is takes a signal of 0, or refuses it as
/// another user's, and none has it (ESRCH).
fn running(pid: Pid) -> bool {
    test_kill_process(pid) != Err(Errno::SRCH)
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command, Stdio};

    use super::*;
    use crate::failpoint::Failpoints;
    use crate::fs::{dir_entries, write_cache_parts};

    /// A process that runs until its standard input is closed.
    fn process() -> Child {
        Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap()
    }

    /// Ends `child`: its standard input closed, and the process waited for, so that its id is no
    /// running process's.
    fn ended(mut child: Child) -> u32 {
        drop(child.stdin.take());
        child.wait().unwrap();
        child.id()
    }

    /// The names in `dir`.
    fn names(dir: &Path) -> Vec<String> {
        (dir_entries(dir).unwrap().iter())
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    /// A write of a cache removes the temporary files of it whose writer is gone, and nothing
    /// else: not one of a writer still running, not another file's, not one bead would not have
    /// named.
    #[test]
    fn a_cache_write_removes_what_writers_that_died_left_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("projection-texts");
        let running = process();
        let dead = ended(process());
        let live = running.id();
        let kept = [
            format!(".projection-texts.{live}.tmp"),
            format!(".projection-texts.0{dead}.tmp"),
            format!(".projection-texts.+{dead}.tmp"),
            format!(".projection-texts.{dead}.tmp.old"),
            format!(".projection.{dead}.tmp"),
            format!("projection-texts.{dead}.tmp"),
            ".projection-texts.x.tmp".to_owned(),
            ".projection-texts.0.tmp".to_owned(),
            ".projection-texts..tmp".to_owned(),
        ];
        let removed = format!(".projection-texts.{dead}.tmp");
        for name in kept.iter().chain([&removed]) {
            std::fs::write(dir.path().join(name), "left behind").unwrap();
        }
        write_cache_parts(&target, &[b"texts"], &Failpoints::none()).unwrap();
        let mut expected: Vec<String> = kept.to_vec();
        expected.push("projection-texts".to_owned());
        expected.sort();
        assert_eq!(names(dir.path()), expected);
        assert_eq!(std::fs::read(&target).unwrap(), b"texts");
        // the writer still running ends: the next write removes its file
        let live = ended(running);
        write_cache_parts(&target, &[b"more"], &Failpoints::none()).unwrap();
        expected.retain(|name| *name != format!(".projection-texts.{live}.tmp"));
        assert_eq!(names(dir.path()), expected);
        assert_eq!(std::fs::read(&target).unwrap(), b"more");
    }

    /// Which names are a process's temporary file for a target: the id as bead writes it, and
    /// a name taken as its bytes, whether or not they are UTF-8.
    #[test]
    fn a_writer_is_read_from_the_name_bead_gives_its_temporary_file() {
        let of = |name: &[u8], target: &[u8]| {
            writer_of(OsStr::from_bytes(name), OsStr::from_bytes(target)).map(Pid::as_raw_nonzero)
        };
        let own = i32::try_from(std::process::id()).unwrap();
        let name = format!(".projection.{own}.tmp");
        assert_eq!(of(name.as_bytes(), b"projection").map(i32::from), Some(own));
        assert_eq!(of(name.as_bytes(), b"projection-texts"), None);
        assert_eq!(
            of(b".projection.1.tmp", b"projection").map(i32::from),
            Some(1)
        );
        for name in [
            &b".projection.01.tmp"[..],
            b".projection.+1.tmp",
            b".projection.-1.tmp",
            b".projection.0.tmp",
            b".projection.1.tmp.old",
            b".projection.2147483648.tmp",
            b".projection.tmp",
            b".projection..tmp",
            b".projection.1\xff.tmp",
            b"projection.1.tmp",
        ] {
            assert_eq!(of(name, b"projection"), None, "{name:?}");
        }
        assert_eq!(of(b".a\xffb.3.tmp", b"a\xffb").map(i32::from), Some(3));
    }
}
