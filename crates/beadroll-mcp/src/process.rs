//! Bounded, nonblocking subprocess execution using `poll(2)` with process-group cleanup on
//! timeout or oversized output.
//!
//! The child leader is never reaped until both output pipes reach EOF or the process group has
//! been signalled, avoiding any PID-reuse window before `kill_process_group`. Nonblocking pipes
//! ensure that even a descendant in a separate session holding stdout or stderr open cannot block
//! timeout recovery.

use std::ffi::OsString;
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::process::{Pid, Signal, kill_process_group};

/// Poll interval while waiting for a child that already closed both pipes to finish exiting.
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Captured output of a completed child process.
pub(crate) struct ProcessOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// Failure modes when spawning or collecting a bounded child process.
pub(crate) enum ProcessError {
    Spawn(io::Error),
    TimedOut { timeout: Duration },
    OutputTooLarge { limit: usize },
}

/// Nonblocking pipe reader tracking EOF and a strict byte cap.
struct PipeReader<R> {
    pipe: Option<R>,
    buf: Vec<u8>,
}

impl<R: Read + AsFd> PipeReader<R> {
    fn new(pipe: Option<R>) -> io::Result<Self> {
        if let Some(ref handle) = pipe {
            let flags = fcntl_getfl(handle).map_err(io::Error::from)?;
            fcntl_setfl(handle, flags | OFlags::NONBLOCK).map_err(io::Error::from)?;
        }
        Ok(Self {
            pipe,
            buf: Vec::new(),
        })
    }

    fn is_open(&self) -> bool {
        self.pipe.is_some()
    }

    /// Drains currently available bytes without blocking. Returns `Err(Overflow)` immediately once
    /// buffered bytes exceed `max_bytes`.
    fn drain_available(&mut self, max_bytes: usize) -> Result<(), PipeDrainError> {
        let mut chunk = [0u8; 8192];
        loop {
            let Some(pipe) = self.pipe.as_mut() else {
                return Ok(());
            };
            match pipe.read(&mut chunk) {
                Ok(0) => {
                    self.pipe = None;
                    return Ok(());
                }
                Ok(n) => {
                    if self.buf.len().saturating_add(n) > max_bytes {
                        self.pipe = None;
                        return Err(PipeDrainError::Overflow);
                    }
                    if let Some(slice) = chunk.get(..n) {
                        self.buf.extend_from_slice(slice);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.pipe = None;
                    return Err(PipeDrainError::Io(error));
                }
            }
        }
    }

    fn close(&mut self) {
        self.pipe = None;
    }
}

enum PipeDrainError {
    Overflow,
    Io(io::Error),
}

/// Spawns `program` with `args` in `cwd` and `env` inside its own process group, drains stdout and
/// stderr nonblockingly up to `max_bytes` each, and enforces `timeout` over both pipe EOF and
/// child exit.
pub(crate) fn run_bounded(
    program: &Path,
    args: &[&str],
    cwd: &Path,
    env: &[(OsString, OsString)],
    timeout: Duration,
    max_bytes: usize,
) -> Result<ProcessOutput, ProcessError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);

    let mut child = cmd.spawn().map_err(ProcessError::Spawn)?;
    let mut stdout = match PipeReader::new(child.stdout.take()) {
        Ok(reader) => reader,
        Err(error) => {
            kill_and_reap(&mut child);
            return Err(ProcessError::Spawn(error));
        }
    };
    let mut stderr = match PipeReader::new(child.stderr.take()) {
        Ok(reader) => reader,
        Err(error) => {
            stdout.close();
            kill_and_reap(&mut child);
            return Err(ProcessError::Spawn(error));
        }
    };

    let deadline = Instant::now() + timeout;
    if let Err(error) = drain_both_until_eof(
        &mut child,
        &mut stdout,
        &mut stderr,
        deadline,
        timeout,
        max_bytes,
    ) {
        // Close our read ends before reaping so an escaped-session descendant holding a write end
        // never stalls timeout or overflow recovery.
        stdout.close();
        stderr.close();
        kill_and_reap(&mut child);
        return Err(error);
    }

    let status = match wait_for_exit_after_eof(&mut child, deadline) {
        Ok(Some(status)) => status,
        Ok(None) => {
            kill_and_reap(&mut child);
            return Err(ProcessError::TimedOut { timeout });
        }
        Err(error) => {
            kill_and_reap(&mut child);
            return Err(ProcessError::Spawn(error));
        }
    };

    Ok(ProcessOutput {
        status,
        stdout: stdout.buf,
        stderr: stderr.buf,
    })
}

fn drain_both_until_eof<R: Read + AsFd, E: Read + AsFd>(
    child: &mut Child,
    stdout: &mut PipeReader<R>,
    stderr: &mut PipeReader<E>,
    deadline: Instant,
    timeout: Duration,
    max_bytes: usize,
) -> Result<(), ProcessError> {
    let _ = child;
    while stdout.is_open() || stderr.is_open() {
        let now = Instant::now();
        if now >= deadline {
            return Err(ProcessError::TimedOut { timeout });
        }
        let remaining = deadline - now;
        let ts = Timespec {
            tv_sec: i64::try_from(remaining.as_secs()).unwrap_or(i64::MAX),
            tv_nsec: i64::from(remaining.subsec_nanos()),
        };

        let mut fds = Vec::with_capacity(2);
        if let Some(ref pipe) = stdout.pipe {
            fds.push(PollFd::new(pipe, PollFlags::IN));
        }
        if let Some(ref pipe) = stderr.pipe {
            fds.push(PollFd::new(pipe, PollFlags::IN));
        }
        match poll(&mut fds, Some(&ts)) {
            Ok(0) => return Err(ProcessError::TimedOut { timeout }),
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(ProcessError::Spawn(io::Error::from(error))),
        }

        drain_checked(stdout, max_bytes)?;
        drain_checked(stderr, max_bytes)?;
    }
    Ok(())
}

fn drain_checked<R: Read + AsFd>(
    reader: &mut PipeReader<R>,
    max_bytes: usize,
) -> Result<(), ProcessError> {
    match reader.drain_available(max_bytes) {
        Ok(()) => Ok(()),
        Err(PipeDrainError::Overflow) => Err(ProcessError::OutputTooLarge { limit: max_bytes }),
        Err(PipeDrainError::Io(error)) => Err(ProcessError::Spawn(error)),
    }
}

fn wait_for_exit_after_eof(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        std::thread::sleep((deadline - now).min(EXIT_POLL_INTERVAL));
    }
}

/// Signals the child's process group while the leader PID is still unreaped (so its PGID cannot
/// have been recycled), then kills and reaps the leader.
fn kill_and_reap(child: &mut Child) {
    let pid = Pid::from_child(child);
    let _ = kill_process_group(pid, Signal::KILL);
    let _ = child.kill();
    let _ = child.wait();
}
