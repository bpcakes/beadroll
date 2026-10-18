//! Runs every git process bead starts. Capture output in unlinked files and wait for git's exit
//! rather than output EOF: hooks and transports may leave background processes holding stderr
//! open. Each git has its own session, away from the caller's terminal. On fetch timeout, kill
//! the process group; a push uses `reap(false)` to kill only the client, allowing the receiver to
//! finish. Ownership settling resolves that uncertain outcome.

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
use crate::text::{strip, strip_ascii};

/// A git process's explicit environment, working directory (`None` inherits the caller's) and
/// output-file directory. The CLI builds the initial environment; [`crate::sync_env`] derives the
/// private repository's environment.
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

    /// The same environment and directory with output files made in `tmp`.
    pub fn with_tmp(&self, tmp: PathBuf) -> Env {
        Env {
            vars: self.vars.clone(),
            dir: self.dir.clone(),
            tmp,
        }
    }

    /// The same environment and output files with git starting in `dir`.
    pub fn with_dir(&self, dir: PathBuf) -> Env {
        Env {
            vars: self.vars.clone(),
            dir: Some(dir),
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

    /// Allows git to run for `seconds`, then kills it and returns [`Error::Timeout`]. A negative
    /// duration permits no wait.
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

    /// Runs git and returns its result regardless of exit status.
    pub fn run(self) -> Result<Output, Error> {
        let (output, _) = self.execute()?;
        Ok(output)
    }

    /// Runs git, returning [`Error::Failed`] on unsuccessful exit.
    pub fn check(self) -> Result<Output, Error> {
        let (output, args) = self.execute()?;
        if output.returncode == 0 {
            return Ok(output);
        }
        Err(failure(&args, &output))
    }

    /// Runs git and returns stdout without surrounding ASCII whitespace. An unsuccessful exit
    /// returns [`Error::Failed`].
    pub fn text(self) -> Result<OsString, Error> {
        let output = self.check()?;
        Ok(OsString::from_vec(strip_ascii(&output.stdout).to_vec()))
    }

    /// The same as [`Git::text`], as a string: git's ids, names and formats are ASCII.
    pub fn string(self) -> Result<String, Error> {
        Ok(self.text()?.to_string_lossy().into_owned())
    }

    /// Whether this git has a time limit.
    pub(crate) fn has_timeout(&self) -> bool {
        self.timeout.is_some()
    }

    fn execute(self) -> Result<(Output, Vec<OsString>), Error> {
        let (command, out, err) = self.prepare()?;
        let waited = wait_for(command, self.timeout, self.reap)?;
        let Some(status) = waited else {
            let first = self.args.first().map(|a| a.to_string_lossy());
            return Err(Error::Timeout {
                command: first.unwrap_or_default().into_owned(),
                seconds: seconds(self.timeout.unwrap_or_default()),
            });
        };
        Ok((finished(status, &out, &err)?, self.args))
    }

    /// The command that runs this git, its output going to the two unlinked files returned with it.
    pub(crate) fn prepare(&self) -> Result<(Command, File, File), Error> {
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
        Ok((command, out, err))
    }
}

/// A timeout as [`Error::Timeout`] gives it: in seconds, to the millisecond and without trailing
/// zeros (`45`, `0.2`), as `BEAD_NET_TIMEOUT` is set.
fn seconds(timeout: f64) -> String {
    let text = format!("{timeout:.3}");
    let text = text.trim_end_matches('0');
    text.strip_suffix('.').unwrap_or(text).to_owned()
}

/// A completed git's status and captured output.
pub(crate) fn finished(status: ExitStatus, out: &File, err: &File) -> Result<Output, Error> {
    let returncode = status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or_default());
    Ok(Output {
        returncode,
        stdout: read_whole(out)?,
        stderr: read_whole(err)?,
    })
}

/// A git result: exit code or negated terminating signal number, plus captured output.
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

    /// Whether a signal ended git rather than its own exit: a failure that says nothing of what
    /// git was reading, since something outside it (an OOM killer, say) cut it short.
    pub fn killed(&self) -> bool {
        self.returncode < 0
    }

    /// The [`Error::Failed`] [`Git::check`] gives when `git ARGS` answered with this output.
    pub fn failed(&self, args: &[&OsStr]) -> Error {
        let args: Vec<OsString> = args.iter().map(|arg| arg.to_os_string()).collect();
        failure(&args, self)
    }

    /// Stdout with surrounding ASCII whitespace removed, decoded with replacement for invalid
    /// UTF-8.
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(strip_ascii(&self.stdout)).into_owned()
    }

    /// What [`Git::text`] gives when `git ARGS` answered with this output: standard output without
    /// the ASCII whitespace at either end, or [`Error::Failed`] naming the command.
    pub fn checked_text(self, args: &[&str]) -> Result<OsString, Error> {
        if self.returncode != 0 {
            let args: Vec<OsString> = args.iter().map(OsString::from).collect();
            return Err(failure(&args, &self));
        }
        Ok(OsString::from_vec(strip_ascii(&self.stdout).to_vec()))
    }
}

/// The error from a failed git: its first two arguments, followed by its diagnostic or
/// terminating signal.
fn failure(args: &[OsString], output: &Output) -> Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut said = strip(&stderr).to_owned();
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
pub(crate) fn kill(pid: u32, reap: bool) {
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

/// Gives the child a separate session and restores the default SIGXFSZ action before exec.
///
/// setsid detaches git and helpers from the caller's process group and controlling terminal,
/// prevents terminal prompts and confines fetch-timeout killing to git's group. Do not call
/// process_group(0) first, because a group leader cannot call setsid. Bead ignores SIGXFSZ and
/// that disposition survives exec, so restoring it ensures git dies when it exceeds the file-size
/// limit.
#[allow(
    unsafe_code,
    reason = "one of the two audited unsafe spots (CONTRIBUTING.md): code that runs in the child between fork and exec"
)]
pub(crate) fn detach(command: &mut Command) {
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

/// Reads `file` from the start without changing the shared offset, allowing surviving git
/// children to keep appending.
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
    fn a_signal_ends_git_killed_and_its_own_exit_does_not() {
        let tmp = std::env::temp_dir();
        let (out, err) = (
            tempfile::tempfile_in(&tmp).unwrap(),
            tempfile::tempfile_in(&tmp).unwrap(),
        );
        // wait statuses: a signal in the low bits, an exit code in the next byte
        let killed = finished(ExitStatus::from_raw(libc::SIGKILL), &out, &err).unwrap();
        assert!(killed.killed() && !killed.success());
        let exited = finished(ExitStatus::from_raw(128 << 8), &out, &err).unwrap();
        assert!(!exited.killed() && !exited.success());
        assert_eq!(exited.returncode, 128);
        let error = killed.failed(&[OsStr::new("cat-file"), OsStr::new("blob")]);
        assert_eq!(error.to_string(), "git cat-file blob: killed by SIGKILL");
    }

    #[test]
    fn a_timeout_reads_in_seconds_to_the_millisecond() {
        let read = [
            (45.0, "45"),
            (10.0, "10"),
            (100.0, "100"),
            (1.0, "1"),
            (2.5, "2.5"),
            (0.2, "0.2"),
            (0.25, "0.25"),
            (0.1234, "0.123"),
            (0.0, "0"),
        ];
        for (timeout, shown) in read {
            assert_eq!(seconds(timeout), shown, "{timeout}");
        }
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
