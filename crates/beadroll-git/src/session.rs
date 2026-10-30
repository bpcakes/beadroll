//! A persistent git process (`crates/beadroll-git/AGENTS.md`), started in an explicit Env with its
//! own session and default SIGXFSZ. Stdin and stdout are pipes; stderr is an unlinked file so it
//! cannot fill an unread pipe. Local calls have no timeout and read only the protocol-announced
//! answer. Dropping the session kills and reaps it and its helpers. If bead exits abruptly, pipe
//! closure gives git EOF.

use std::ffi::OsStr;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::KillScope;
use crate::runner::{Env, detach, kill};

/// The longest line a protocol answer may have: an object's header, a name, `commit: ok`.
const LINE_LIMIT: u64 = 4096;

/// A persistent git could not start, exited or violated its protocol. The caller falls back to a
/// standalone git invocation and returns that command's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the long-lived git gave no usable answer")]
pub struct NoAnswer;

/// One long-lived git, spoken to through its standard input and output.
#[derive(Debug)]
pub(crate) struct Session {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Session {
    /// `git ARGS` started in `env`, waiting for its first question.
    pub(crate) fn start<I, S>(env: &Env, args: I) -> Result<Session, NoAnswer>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let errors = tempfile::tempfile_in(env.tmp()).map_err(|_| NoAnswer)?;
        let mut command = Command::new("git");
        command
            .args(args)
            .env_clear()
            .envs(env.vars())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(errors);
        if let Some(dir) = env.dir() {
            command.current_dir(dir);
        }
        detach(&mut command);
        let mut child = command.spawn().map_err(|_| NoAnswer)?;
        drop(command); // our copy of its standard error goes now
        match (child.stdin.take(), child.stdout.take()) {
            (Some(input), Some(output)) => Ok(Session {
                child,
                input,
                output: BufReader::new(output),
            }),
            _ => {
                end(&mut child);
                Err(NoAnswer)
            }
        }
    }

    /// Writes `data` to git and flushes it.
    pub(crate) fn send(&mut self, data: &[u8]) -> Result<(), NoAnswer> {
        self.input
            .write_all(data)
            .and_then(|()| self.input.flush())
            .map_err(|_| NoAnswer)
    }

    /// The next line git writes, without its newline.
    pub(crate) fn line(&mut self) -> Result<Vec<u8>, NoAnswer> {
        let mut line = Vec::new();
        (&mut self.output)
            .take(LINE_LIMIT)
            .read_until(b'\n', &mut line)
            .map_err(|_| NoAnswer)?;
        if line.pop() != Some(b'\n') {
            return Err(NoAnswer); // gone, or a line longer than any answer
        }
        Ok(line)
    }

    /// The next `size` bytes git writes, read as they come: never more than git has sent.
    pub(crate) fn exact(&mut self, size: u64) -> Result<Vec<u8>, NoAnswer> {
        let mut data = Vec::new();
        (&mut self.output)
            .take(size)
            .read_to_end(&mut data)
            .map_err(|_| NoAnswer)?;
        if data.len() as u64 != size {
            return Err(NoAnswer);
        }
        Ok(data)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        end(&mut self.child);
    }
}

/// Kills git with every helper it started (its own session is its process group) and reaps it.
fn end(child: &mut Child) {
    kill(child.id(), KillScope::ProcessGroup);
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env::new(std::env::vars_os(), None, std::env::temp_dir())
    }

    #[test]
    fn a_session_answers_question_after_question_and_is_reaped_when_dropped() {
        let mut session = Session::start(&env(), ["hash-object", "--stdin-paths"]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello");
        std::fs::write(&file, b"hello\n").unwrap();
        for _ in 0..3 {
            let mut request = file.as_os_str().as_encoded_bytes().to_vec();
            request.push(b'\n');
            session.send(&request).unwrap();
            assert_eq!(
                session.line().unwrap(),
                b"ce013625030ba8dba906f756967f9e9ca394464a"
            );
        }
        let pid = i32::try_from(session.child.id()).unwrap();
        drop(session);
        let gone = rustix::process::Pid::from_raw(pid)
            .is_none_or(|pid| rustix::process::test_kill_process(pid).is_err());
        assert!(gone, "the session's git outlived it");
    }

    #[test]
    fn a_git_that_dies_answers_nothing() {
        let mut session = Session::start(&env(), ["hash-object", "--stdin-paths"]).unwrap();
        session.send(b"/nonexistent/bead/file\n").unwrap();
        assert_eq!(session.line(), Err(NoAnswer));
        assert_eq!(session.exact(1), Err(NoAnswer));
    }

    #[test]
    fn a_git_that_cannot_start_is_no_answer() {
        let empty = Env::new(
            [(
                std::ffi::OsString::from("PATH"),
                std::ffi::OsString::from("/nonexistent"),
            )],
            None,
            std::env::temp_dir(),
        );
        assert_eq!(
            Session::start(&empty, ["cat-file", "--batch"]).err(),
            Some(NoAnswer)
        );
    }
}
