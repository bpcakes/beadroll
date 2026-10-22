//! The temporary files a cache's writers left beside it when they died (roll-lqjj.52):
//! `.NAME.PID.tmp` beside NAME, with the id of a process that is gone. A bead killed between the
//! write of one and its rename into place (a crash, a SIGKILL) leaves it, as large as the file,
//! and nothing else would ever remove it, since its name is that process's own: the next
//! write-back of the projection does, both caches' at once, before it writes either
//! (`projection::save`), since the text file is appended to in place and started again only now
//! and then, where the projection is written whole every time.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use rustix::io::Errno;
use rustix::process::{Pid, test_kill_process};

/// Tries to remove the temporary files beside each of `paths` whose process id no longer exists,
/// a directory several of them share listed once. A process id still present, including one
/// reused by another process, keeps its file at this check. This assumes the store's writers
/// share one host and PID namespace. A directory that cannot be listed, or a file that cannot be
/// removed, is left as it is: what a cache's writers left is no failure of this one.
pub(crate) fn remove(paths: &[&Path]) {
    let mut listed: Vec<&Path> = Vec::new();
    for dir in paths.iter().map(|path| parent(path)) {
        if listed.contains(&dir) {
            continue;
        }
        listed.push(dir);
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let targets: Vec<&OsStr> = (paths.iter())
            .filter(|path| parent(path) == dir)
            .filter_map(|path| path.file_name())
            .collect();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let abandoned = (targets.iter())
                .any(|target| writer_of(&name, target).is_some_and(|pid| !running(pid)));
            if abandoned {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// The directory `path` is in.
fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("."))
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

/// Processes for the tests to name temporary files after: one running, one ended.
#[cfg(test)]
pub(crate) mod testing {
    use std::process::{Child, Command, Stdio};

    /// A process that runs until its standard input is closed.
    pub(crate) fn process() -> Child {
        Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap()
    }

    /// Ends `child`: its standard input closed, and the process waited for, so that its id is no
    /// running process's.
    pub(crate) fn ended(mut child: Child) -> u32 {
        drop(child.stdin.take());
        child.wait().unwrap();
        child.id()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{ended, process};
    use super::*;
    use crate::fs::dir_entries;

    /// The names in `dir`.
    fn names(dir: &Path) -> Vec<String> {
        (dir_entries(dir).unwrap().iter())
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    /// A sweep of the two caches removes the temporary files of either whose writer is gone, and
    /// nothing else: not one of a writer still running, not another file's, not one bead would
    /// not have named.
    #[test]
    fn a_sweep_removes_what_writers_that_died_left_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let caches = [
            dir.path().join("projection"),
            dir.path().join("projection-texts"),
        ];
        let running = process();
        let dead = ended(process());
        let live = running.id();
        let kept = [
            format!(".projection-texts.{live}.tmp"),
            format!(".projection.{live}.tmp"),
            format!(".projection-texts.0{dead}.tmp"),
            format!(".projection-texts.+{dead}.tmp"),
            format!(".projection-texts.{dead}.tmp.old"),
            format!(".meta.json.{dead}.tmp"),
            format!("projection-texts.{dead}.tmp"),
            ".projection-texts.x.tmp".to_owned(),
            ".projection-texts.0.tmp".to_owned(),
            ".projection-texts..tmp".to_owned(),
            "projection".to_owned(),
            "projection-texts".to_owned(),
        ];
        let removed = [
            format!(".projection-texts.{dead}.tmp"),
            format!(".projection.{dead}.tmp"),
        ];
        for name in kept.iter().chain(&removed) {
            std::fs::write(dir.path().join(name), "left behind").unwrap();
        }
        remove(&[&caches[0], &caches[1]]);
        let mut expected: Vec<String> = kept.to_vec();
        expected.sort();
        assert_eq!(names(dir.path()), expected);
        // the writer still running ends: the next sweep removes its files
        let live = ended(running);
        remove(&[&caches[0], &caches[1]]);
        expected.retain(|name| !name.contains(&format!(".{live}.tmp")));
        assert_eq!(names(dir.path()), expected);
        // a directory that is not there, or a path without a name, is nothing to sweep
        remove(&[&dir.path().join("gone/projection"), Path::new("/")]);
        assert_eq!(names(dir.path()), expected);
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
