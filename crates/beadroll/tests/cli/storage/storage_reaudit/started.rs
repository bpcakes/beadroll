//! Starting, waiting for and stopping `bead` beside the test, and the
//! `pause-until` gates the test stops it at.

use std::io::Read;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use beadroll_test_support::{Invocation, Run, Sandbox};
use rustix::process::{Pid, Signal, kill_process_group};

/// The sandbox handshake for `pause-until(PATH)`: bead creates PATH.ready at the boundary and
/// resumes when PATH.go exists.
pub(super) struct Gate(PathBuf);

impl Gate {
    /// A gate named `name` in the sandbox.
    pub(super) fn new(sandbox: &Sandbox, name: &str) -> Gate {
        Gate(sandbox.root().join(name))
    }

    /// `BEAD_FAILPOINTS` for `point`, with its selector, stopping at this gate.
    pub(super) fn at(&self, point: &str) -> String {
        format!("{point}=pause-until({})", self.0.display())
    }

    fn file(&self, suffix: &str) -> PathBuf {
        let mut name = self.0.clone().into_os_string();
        name.push(suffix);
        PathBuf::from(name)
    }

    /// The file bead touches once it has stopped at this gate.
    pub(super) fn ready(&self) -> PathBuf {
        self.file(".ready")
    }

    /// Lets bead go on.
    pub(super) fn open(&self) {
        std::fs::write(self.file(".go"), b"").expect("the gate's go file");
    }
}

/// A `bead` started beside the test: nothing on its standard input, its output drained by
/// separate threads, and its own process group so a kill reaches its children too.
/// One still running when dropped is killed and reaped.
pub(super) struct Started {
    child: Child,
    output: Option<(Drained, Drained)>,
}

/// What a pipe held, handed over by the thread that read it to its end.
type Drained = Receiver<Vec<u8>>;

impl Started {
    /// Starts `invocation`.
    pub(super) fn start(invocation: Invocation<'_>) -> Started {
        let mut command = invocation.command();
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command.spawn().expect("bead starts");
        let stdout = drain(child.stdout.take().expect("bead's standard output"));
        let stderr = drain(child.stderr.take().expect("bead's standard error"));
        Started {
            child,
            output: Some((stdout, stderr)),
        }
    }

    /// `poll()`: how it ended, or `None` while it runs.
    fn poll(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().expect("the process's status")
    }

    /// Sends SIGKILL to the child's process group.
    pub(super) fn kill(&self) {
        let group = Pid::from_child(&self.child);
        kill_process_group(group, Signal::KILL).expect("its process group");
    }

    /// The process status and output, or `None` if it has not finished within `timeout`.
    pub(super) fn within(&mut self, timeout: Duration) -> Option<Run> {
        let until = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = self.poll() {
                break status;
            }
            if Instant::now() >= until {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let (stdout, stderr) = self.output.take().expect("output not yet read");
        Some(Run {
            status,
            stdout: received(&stdout, "standard output"),
            stderr: received(&stderr, "standard error"),
        })
    }

    /// The process status and output; exceeding `timeout` fails the test.
    pub(super) fn communicate(&mut self, timeout: Duration) -> Run {
        let run = self.within(timeout);
        run.unwrap_or_else(|| panic!("bead had not ended after {timeout:?}"))
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = kill_process_group(Pid::from_child(&self.child), Signal::KILL);
            let _ = self.child.wait();
        }
    }
}

/// A thread that reads `pipe` to its end, and the channel it hands what it read to.
fn drain(mut pipe: impl Read + Send + 'static) -> Drained {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
}

/// What bead printed on `stream`, once the pipe has ended, decoded as `text=True` decodes it: a
/// pipe still open five seconds after bead ended, or output that is not UTF-8, fails the test.
fn received(receiver: &Drained, stream: &str) -> String {
    let bytes = receiver.recv_timeout(Duration::from_secs(5));
    let bytes = bytes.unwrap_or_else(|_| panic!("bead's {stream} had not ended"));
    String::from_utf8(bytes).unwrap_or_else(|error| panic!("bead's {stream} is not UTF-8: {error}"))
}

/// Waits up to ten seconds for `marker`. The process quitting first fails the
/// test with what it printed, and so does a marker that never comes.
pub(super) fn wait(marker: &Path, process: &mut Started) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        if process.poll().is_some() {
            let printed = process.communicate(Duration::from_secs(10));
            panic!("worker quit before {}: {printed:?}", marker.display());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "{}", marker.display());
}
