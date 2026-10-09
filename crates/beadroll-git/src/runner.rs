//! The git runner (Python's `run_git` and `git_text`): every git process bead starts.
//!
//! git's answer is its exit status, so the runner waits for git, not for the end of its output:
//! that goes to unlinked temporary files, because a helper git started (a hook's or a transport's
//! background job) may keep git's stderr open long after git has exited. Each git runs in a session
//! of its own, away from the caller's controlling terminal. On a timeout, `reap` kills git together
//! with the helpers it started (a fetch has nothing in flight worth keeping); a push passes
//! `reap(false)`, so only the client dies and whatever the receiving side is doing finishes or
//! fails on its own: that is the uncertain outcome the ownership protocol settles.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Seek as _, SeekFrom, Write as _};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::FileExt as _;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::error::{Error, IoError};
use crate::signal::signal_name;
use crate::text::{py_strip, strip_ascii};

/// The world a git process starts in: its whole environment, the directory it starts in (`None`:
/// the caller's own, as Python's `subprocess` leaves it), and the directory its output files are
/// made in (Python's `tempfile.gettempdir()`, normally `TMPDIR`). The CLI builds the first one from
/// the process; [`crate::sync_env`] derives `sync.git`'s from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Env {
    vars: BTreeMap<OsString, OsString>,
    dir: Option<PathBuf>,
    tmp: PathBuf,
}

impl Env {
    /// An environment of exactly `vars`, starting in `dir`, with output files in `tmp`.
    pub fn new(
        vars: impl IntoIterator<Item = (OsString, OsString)>,
        dir: Option<PathBuf>,
        tmp: PathBuf,
    ) -> Env {
        Env {
            vars: vars.into_iter().collect(),
            dir,
            tmp,
        }
    }

    /// The variable `name`, if it is set (to anything, the empty string included).
    pub fn var(&self, name: impl AsRef<OsStr>) -> Option<&OsStr> {
        self.vars.get(name.as_ref()).map(OsString::as_os_str)
    }

    /// Every variable.
    pub fn vars(&self) -> &BTreeMap<OsString, OsString> {
        &self.vars
    }

    /// The directory git starts in, when it is not the caller's.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Where the files that take git's output are made.
    pub fn tmp(&self) -> &Path {
        &self.tmp
    }

    /// The same directories with exactly `vars`.
    pub fn with_vars(&self, vars: BTreeMap<OsString, OsString>) -> Env {
        Env {
            vars,
            dir: self.dir.clone(),
            tmp: self.tmp.clone(),
        }
    }

    /// `git ARGS` in this environment, to adjust and run.
    pub fn git<I, S>(&self, args: I) -> Git<'_>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Git {
            env: self,
            args: args.into_iter().map(|a| a.as_ref().to_owned()).collect(),
            input: None,
            timeout: None,
            reap: true,
        }
    }
}

/// One git invocation: its arguments, what it reads on standard input, how long it may take, and
/// what dies when it takes too long.
#[derive(Debug)]
pub struct Git<'a> {
    env: &'a Env,
    args: Vec<OsString>,
    input: Option<Vec<u8>>,
    timeout: Option<f64>,
    reap: bool,
}

impl Git<'_> {
    /// git reads `data` on its standard input (otherwise `/dev/null`).
    pub fn input(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.input = Some(data.into());
        self
    }

    /// git may run for `seconds` (Python's `timeout`); after that it is killed and the call fails
    /// with [`Error::Timeout`]. A negative time is no time at all, as Python's `Thread.join` reads it.
    pub fn timeout(mut self, seconds: f64) -> Self {
        self.timeout = Some(seconds);
        self
    }

    /// Whether a timeout kills git's whole process group (`true`, the default, for a fetch) or the
    /// client alone (`false`, for a push).
    pub fn reap(mut self, reap: bool) -> Self {
        self.reap = reap;
        self
    }

    /// Runs git and returns what it did, whatever its exit status (Python's `check=False`).
    pub fn run(self) -> Result<Output, Error> {
        let (output, _) = self.execute()?;
        Ok(output)
    }

    /// Runs git; a failure is [`Error::Failed`] (Python's `check=True`).
    pub fn check(self) -> Result<Output, Error> {
        let (output, args) = self.execute()?;
        if output.returncode == 0 {
            return Ok(output);
        }
        Err(failure(&args, &output))
    }

    /// Runs git and returns its standard output without the ASCII whitespace at either end, as
    /// Python's `git_text` does; a failure is [`Error::Failed`].
    pub fn text(self) -> Result<OsString, Error> {
        let output = self.check()?;
        Ok(OsString::from_vec(strip_ascii(&output.stdout).to_vec()))
    }

    /// The same as [`Git::text`], as a string: git's ids, names and formats are ASCII.
    pub fn string(self) -> Result<String, Error> {
        Ok(self.text()?.to_string_lossy().into_owned())
    }

    fn execute(self) -> Result<(Output, Vec<OsString>), Error> {
        let tmp = &self.env.tmp;
        let unlinked = || tempfile::tempfile_in(tmp).map_err(|e| Error::Io(IoError::bare(&e)));
        let out = unlinked()?;
        let err = unlinked()?;
        let stdin = match &self.input {
            Some(data) => {
                let mut file = unlinked()?;
                let written = file
                    .write_all(data)
                    .and_then(|()| file.seek(SeekFrom::Start(0)).map(drop));
                written.map_err(|e| Error::Io(IoError::bare(&e)))?;
                Stdio::from(file)
            }
            None => Stdio::null(),
        };
        let clone = |file: &File| file.try_clone().map_err(|e| Error::Io(IoError::bare(&e)));
        let mut command = Command::new("git");
        command
            .args(&self.args)
            .env_clear()
            .envs(&self.env.vars)
            .stdin(stdin)
            .stdout(clone(&out)?)
            .stderr(clone(&err)?);
        if let Some(dir) = &self.env.dir {
            command.current_dir(dir);
        }
        detach(&mut command);
        let waited = wait_for(command, self.timeout, self.reap)?;
        let Some(status) = waited else {
            let first = self.args.first().map(|a| a.to_string_lossy());
            return Err(Error::Timeout {
                command: first.unwrap_or_default().into_owned(),
                seconds: format!("{:.0}", self.timeout.unwrap_or_default()),
            });
        };
        let returncode = status
            .code()
            .unwrap_or_else(|| -status.signal().unwrap_or_default());
        let output = Output {
            returncode,
            stdout: read_whole(&out)?,
            stderr: read_whole(&err)?,
        };
        Ok((output, self.args))
    }
}

/// What a git process did: its exit status as Python reports it (the negated signal number when a
/// signal killed it), and everything it wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// The exit status, or minus the signal that ended it.
    pub returncode: i32,
    /// Its standard output.
    pub stdout: Vec<u8>,
    /// Its standard error.
    pub stderr: Vec<u8>,
}

impl Output {
    /// Whether git exited with status 0.
    pub fn success(&self) -> bool {
        self.returncode == 0
    }

    /// Standard output without the ASCII whitespace at either end, decoded as Python's `decode()`
    /// with `errors="replace"`.
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(strip_ascii(&self.stdout)).into_owned()
    }
}

/// The error a failed git reports (Python's `check=True`): what git said, or the signal that killed
/// it, after git's first two arguments.
fn failure(args: &[OsString], output: &Output) -> Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut said = py_strip(&stderr).to_owned();
    if output.returncode < 0 {
        let name = signal_name(-output.returncode);
        said = if said.is_empty() {
            format!("killed by {name}")
        } else {
            format!("killed by {name}: {said}")
        };
    }
    let command: Vec<String> = args
        .iter()
        .take(2)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    Error::Failed {
        command: command.join(" "),
        said,
    }
}

/// Starts `command` and waits for it: `Some(status)`, or `None` when it took longer than `timeout`
/// seconds and was killed (its group with `reap`, else the client alone) and then reaped.
fn wait_for(
    mut command: Command,
    timeout: Option<f64>,
    reap: bool,
) -> Result<Option<ExitStatus>, Error> {
    // The waiter exists before git does, so that a git is never started without someone to reap it:
    // a blocking wait on its own thread, not a loop of sleeps.
    let (give, child_slot) = mpsc::channel::<Child>();
    let (tell, done) = mpsc::channel::<io::Result<ExitStatus>>();
    let waiter = std::thread::Builder::new()
        .name("git-waiter".into())
        .spawn(move || {
            if let Ok(mut child) = child_slot.recv() {
                let _ = tell.send(child.wait());
            }
        })
        .map_err(|e| Error::Io(IoError::bare(&e)))?;
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            drop(give);
            let _ = waiter.join();
            return Err(Error::Io(IoError::at(&error, "git")));
        }
    };
    let pid = child.id();
    drop(command); // our copies of git's output files and input go now
    if give.send(child).is_err() {
        return Err(Error::Io(IoError::bare(&io::Error::other(
            "the git waiter thread is gone",
        ))));
    }
    let status = match timeout {
        None => done.recv(),
        Some(seconds) => {
            let limit = Duration::try_from_secs_f64(seconds.max(0.0)).ok();
            match limit.map(|limit| done.recv_timeout(limit)) {
                None => done.recv(),
                Some(Ok(status)) => Ok(status),
                Some(Err(mpsc::RecvTimeoutError::Disconnected)) => Err(mpsc::RecvError),
                Some(Err(mpsc::RecvTimeoutError::Timeout)) => {
                    kill(pid, reap);
                    let _ = done.recv(); // dead now: reaped, so it leaves no zombie
                    let _ = waiter.join();
                    return Ok(None);
                }
            }
        }
    };
    let _ = waiter.join();
    let status = status.map_err(|_| {
        Error::Io(IoError::bare(&io::Error::other(
            "the git waiter thread is gone",
        )))
    })?;
    status.map(Some).map_err(|e| Error::Io(IoError::bare(&e)))
}

/// Kills git (with `reap`, its whole process group: git and every helper it started) after a
/// timeout. Already gone is fine.
fn kill(pid: u32, reap: bool) {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return;
    };
    let signal = rustix::process::Signal::KILL;
    let _ = if reap {
        rustix::process::kill_process_group(pid, signal)
    } else {
        rustix::process::kill_process(pid, signal)
    };
}

/// Gives the child a session of its own and SIGXFSZ's default action back before it runs git.
///
/// `setsid` matches Python's `start_new_session=True`: git and its helpers leave the caller's
/// process group and controlling terminal, so a transport can never prompt on the terminal, and the
/// group a timed-out fetch kills is git's alone. No `process_group(0)` comes first: a process group
/// leader cannot call `setsid`. bead ignores SIGXFSZ as CPython does, and ignored signals survive
/// `exec`, so git would otherwise write on past a file-size limit instead of dying of it.
#[allow(
    unsafe_code,
    reason = "one of the two audited unsafe spots (docs/RUST-PORT.md §2.2): code that runs in the child between fork and exec"
)]
fn detach(command: &mut Command) {
    // SAFETY: the closure runs in the forked child before exec, where only async-signal-safe calls
    // are allowed. It calls setsid(2) and signal(2), both async-signal-safe, and builds an
    // io::Error from errno, which neither allocates nor takes a lock. It touches no memory shared
    // with the parent's other threads.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            libc::signal(libc::SIGXFSZ, libc::SIG_DFL);
            Ok(())
        });
    }
}

/// Everything in `file`, read from its start without moving its offset, which git's children share:
/// one still running writes on after it (Python's `os.pread`).
fn read_whole(file: &File) -> Result<Vec<u8>, Error> {
    let io = |e: io::Error| Error::Io(IoError::bare(&e));
    let size = file.metadata().map_err(io)?.len();
    let mut data = vec![0; usize::try_from(size).unwrap_or(usize::MAX)];
    file.read_exact_at(&mut data, 0).map_err(io)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env::new(std::env::vars_os(), None, std::env::temp_dir())
    }

    #[test]
    fn a_failure_names_the_command_and_what_git_said() {
        let error = env()
            .git(["rev-parse", "--verify", "refs/nothing/here"])
            .check()
            .unwrap_err();
        match error {
            Error::Failed { command, said } => {
                assert_eq!(command, "rev-parse --verify");
                assert!(said.contains("fatal") || said.contains("Needed"), "{said}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn input_reaches_git_and_output_comes_back_whole() {
        let sha = env()
            .git(["hash-object", "--stdin"])
            .input(b"hello\n".to_vec())
            .string()
            .unwrap();
        assert_eq!(sha, "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    #[test]
    fn a_signal_death_reads_killed_by_its_name() {
        let shell = Env::new(
            [(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))],
            None,
            std::env::temp_dir(),
        );
        let error = shell
            .git(["-c", "alias.die=!kill -9 $$", "die"])
            .check()
            .unwrap_err();
        let Error::Failed { said, .. } = error else {
            panic!("{error:?}");
        };
        // git reports its alias's death itself, or dies of the same signal.
        assert!(
            said.contains("SIGKILL") || said.contains("signal"),
            "{said}"
        );
    }

    #[test]
    fn a_missing_git_is_an_io_error_naming_git() {
        let empty = Env::new(
            [(OsString::from("PATH"), OsString::from("/nonexistent"))],
            None,
            std::env::temp_dir(),
        );
        let error = empty.git(["--version"]).run().unwrap_err();
        assert_eq!(error.to_string(), "No such file or directory: git");
    }
}
