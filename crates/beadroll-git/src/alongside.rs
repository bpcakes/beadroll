//! Two local gits at once (`docs/RUST-PORT.md` Appendix B): finding the workspace asks the
//! combined `rev-parse` and origin's `config`, and neither waits for the other.
//!
//! Both are started from the calling thread, one after the other, so that neither can inherit a
//! descriptor the other's start had not yet marked close-on-exec; each runs as the runner runs
//! every git, and the one started first is killed and reaped if it is never waited for.

use std::fs::File;
use std::io;
use std::process::Child;

use crate::error::{Error, IoError};
use crate::runner::{Git, Output, finished, kill};

impl Git<'_> {
    /// Runs this git and `other` at once, each as [`Git::run`] runs it: `other` started first, then
    /// this one run to its end, then `other` waited for. For two local questions; one with a time
    /// limit runs on its own, in turn.
    pub fn run_alongside(self, other: Git<'_>) -> (Result<Output, Error>, Result<Output, Error>) {
        if self.has_timeout() || other.has_timeout() {
            return (self.run(), other.run());
        }
        let started = other.prepare().and_then(|(mut command, out, err)| {
            let child = command
                .spawn()
                .map_err(|e| Error::Io(IoError::at(&e, "git")))?;
            Ok(Running {
                child: Some(child),
                out,
                err,
            })
        });
        (self.run(), started.and_then(Running::finish))
    }
}

/// A git started by [`Git::run_alongside`] and not yet waited for; dropped unfinished, it is killed
/// with its helpers and reaped.
struct Running {
    child: Option<Child>,
    out: File,
    err: File,
}

impl Running {
    /// Waits for git and returns what it did.
    fn finish(mut self) -> Result<Output, Error> {
        let status = match self.child.take() {
            Some(mut child) => child.wait(),
            None => Err(io::Error::other("the git was already waited for")),
        };
        let status = status.map_err(|e| Error::Io(IoError::bare(&e)))?;
        finished(status, &self.out, &self.err)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            kill(child.id(), true);
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use crate::runner::Env;

    #[test]
    fn two_gits_run_alongside_answer_as_each_run_alone() {
        let env = Env::new(std::env::vars_os(), None, std::env::temp_dir());
        let first = ["hash-object", "--stdin"];
        let second = ["rev-parse", "--verify", "refs/nothing/here"];
        let (a, b) = env
            .git(first)
            .input(b"hello\n".to_vec())
            .run_alongside(env.git(second));
        let alone = env.git(first).input(b"hello\n".to_vec()).run();
        assert_eq!(a.unwrap(), alone.unwrap());
        assert_eq!(b.unwrap(), env.git(second).run().unwrap());
        let empty = Env::new(
            [(OsString::from("PATH"), OsString::from("/nonexistent"))],
            None,
            std::env::temp_dir(),
        );
        let (a, b) = empty
            .git(["--version"])
            .run_alongside(empty.git(["--version"]));
        assert_eq!(a.unwrap_err().to_string(), "No such file or directory: git");
        assert_eq!(b.unwrap_err().to_string(), "No such file or directory: git");
    }
}
