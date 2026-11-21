//! Bounded subprocess execution with process-group cleanup on timeout or oversized output.

use std::ffi::OsString;
use std::io::{self, Read};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};

/// Poll interval while waiting for a child process under a deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

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

/// Outcome of reading a single pipe up to `limit` bytes.
enum StreamResult {
    Ok(Vec<u8>),
    Overflow,
    Io(io::Error),
}

/// Spawns `program` with `args` in `cwd` and `env` inside its own process group, captures stdout
/// and stderr up to `max_bytes` each, and enforces `timeout` over both process exit and pipe
/// closure (cleaning up the entire process group if a child or descendant stalls).
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
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_reader = stdout_pipe.map(|pipe| spawn_reader(pipe, max_bytes));
    let stderr_reader = stderr_pipe.map(|pipe| spawn_reader(pipe, max_bytes));

    let deadline = Instant::now() + timeout;
    let status = match wait_for_exit(&mut child, deadline) {
        Ok(Some(status)) => status,
        Ok(None) => {
            kill_and_reap(&mut child);
            join_reader(stdout_reader);
            join_reader(stderr_reader);
            return Err(ProcessError::TimedOut { timeout });
        }
        Err(error) => {
            kill_and_reap(&mut child);
            join_reader(stdout_reader);
            join_reader(stderr_reader);
            return Err(ProcessError::Spawn(error));
        }
    };

    // Even after the direct child exits, a descendant could hold stdout or stderr open. Wait for
    // both reader threads within the remaining deadline, and kill the process group if either
    // hangs or exceeds `max_bytes`.
    let stdout = match collect_stream(stdout_reader, deadline, &mut child, timeout, max_bytes) {
        Ok(bytes) => bytes,
        Err(error) => {
            join_reader(stderr_reader);
            return Err(error);
        }
    };
    let stderr = collect_stream(stderr_reader, deadline, &mut child, timeout, max_bytes)?;

    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
    })
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    max_bytes: usize,
) -> (Receiver<StreamResult>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => {
                    let _ = tx.send(StreamResult::Ok(buf));
                    return;
                }
                Ok(n) => {
                    if buf.len().saturating_add(n) > max_bytes {
                        let _ = tx.send(StreamResult::Overflow);
                        return;
                    }
                    if let Some(slice) = chunk.get(..n) {
                        buf.extend_from_slice(slice);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = tx.send(StreamResult::Io(error));
                    return;
                }
            }
        }
    });
    (rx, handle)
}

fn wait_for_exit(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        let sleep = (deadline - now).min(POLL_INTERVAL);
        thread::sleep(sleep);
    }
}

fn collect_stream(
    reader: Option<(Receiver<StreamResult>, JoinHandle<()>)>,
    deadline: Instant,
    child: &mut Child,
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, ProcessError> {
    let Some((rx, handle)) = reader else {
        return Ok(Vec::new());
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    let result = match rx.recv_timeout(remaining) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => {
            kill_and_reap(child);
            let _ = handle.join();
            return Err(ProcessError::TimedOut { timeout });
        }
        Err(RecvTimeoutError::Disconnected) => {
            let _ = handle.join();
            return Ok(Vec::new());
        }
    };
    match result {
        StreamResult::Ok(bytes) => {
            let _ = handle.join();
            Ok(bytes)
        }
        StreamResult::Overflow => {
            kill_and_reap(child);
            let _ = handle.join();
            Err(ProcessError::OutputTooLarge { limit: max_bytes })
        }
        StreamResult::Io(error) => {
            let _ = handle.join();
            Err(ProcessError::Spawn(error))
        }
    }
}

fn kill_and_reap(child: &mut Child) {
    let pid = Pid::from_child(child);
    let _ = kill_process_group(pid, Signal::KILL);
    let _ = child.kill();
    let _ = child.wait();
}

fn join_reader(reader: Option<(Receiver<StreamResult>, JoinHandle<()>)>) {
    if let Some((_, handle)) = reader {
        let _ = handle.join();
    }
}
