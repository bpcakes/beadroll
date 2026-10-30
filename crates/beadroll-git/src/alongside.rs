//! Local gits at once: finding the workspace asks the combined `rev-parse`, origin's `config` and,
//! where the tree may lack `.beadroll.json`, the committed copy's `cat-file`, and none waits for
//! another.
//!
//! All are started from the calling thread, one after the other, so that none can inherit a
//! descriptor another's start had not yet marked close-on-exec; each runs as the runner runs every
//! git, and one started before is killed and reaped if it is never waited for.

use std::fs::File;
use std::io;
use std::process::Child;

use crate::KillScope;
use crate::error::{Error, IoError};
use crate::runner::{Git, Output, finished, kill};

impl Git<'_> {
    /// Runs this git and `others` at once, each as [`Git::run`] runs it: `others` started first, in
    /// order, then this one run to its end, then `others` waited for. For local questions; when one
    /// has a time limit, each runs on its own, in turn.
    pub fn run_alongside<const N: usize>(
        self,
        others: [Git<'_>; N],
    ) -> (Result<Output, Error>, [Result<Output, Error>; N]) {
        if self.has_timeout() || others.iter().any(Git::has_timeout) {
            return (self.run(), others.map(Git::run));
        }
        let started = others.map(|other| Running::start(&other));
        (
            self.run(),
            started.map(|other| other.and_then(Running::finish)),
        )
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
    /// Starts `git`, to be waited for later.
    fn start(git: &Git<'_>) -> Result<Running, Error> {
        let (mut command, out, err) = git.prepare()?;
        let child = command
            .spawn()
            .map_err(|e| Error::Io(IoError::at(&e, "git")))?;
        Ok(Running {
            child: Some(child),
            out,
            err,
        })
    }

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
            kill(child.id(), KillScope::ProcessGroup);
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use crate::runner::Env;

    #[test]
    fn gits_run_alongside_answer_as_each_run_alone() {
        let env = Env::new(std::env::vars_os(), None, std::env::temp_dir());
        let first = ["hash-object", "--stdin"];
        let second = ["rev-parse", "--verify", "refs/nothing/here"];
        let third = ["hash-object", "--stdin", "-t", "blob"];
        let (a, [b, c]) = env
            .git(first)
            .input(b"hello\n".to_vec())
            .run_alongside([env.git(second), env.git(third).input(b"other\n".to_vec())]);
        let alone = env.git(first).input(b"hello\n".to_vec()).run();
        assert_eq!(a.unwrap(), alone.unwrap());
        assert_eq!(b.unwrap(), env.git(second).run().unwrap());
        let alone = env.git(third).input(b"other\n".to_vec()).run();
        assert_eq!(c.unwrap(), alone.unwrap());
        let empty = Env::new(
            [(OsString::from("PATH"), OsString::from("/nonexistent"))],
            None,
            std::env::temp_dir(),
        );
        let (a, [b]) = empty
            .git(["--version"])
            .run_alongside([empty.git(["--version"])]);
        assert_eq!(a.unwrap_err().to_string(), "No such file or directory: git");
        assert_eq!(b.unwrap_err().to_string(), "No such file or directory: git");
    }
}
