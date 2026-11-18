//! `bead` started beside the test rather than run to its end: nothing on its standard input, its
//! output drained by threads of its own so that a full pipe never holds it up, waited for under a
//! deadline, and killed and reaped if the test drops it still running.

use std::io::Read;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process, kill_process_group};

use crate::{Invocation, Run};

/// Whether a started `bead` leads a process group of its own, which a kill then reaches whole
/// (bead and every git it started), or stays in the test's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessGroup {
    /// A group of its own: [`Spawned::kill`] kills it and its children.
    Own,
    /// The test's group: [`Spawned::kill`] kills bead alone.
    Inherited,
}

/// A `bead` started with [`Invocation::spawn`].
pub struct Spawned {
    child: Child,
    group: ProcessGroup,
    output: Option<(Drained, Drained)>,
}

/// What a pipe held, handed over by the thread that read it to its end.
pub type Drained = Receiver<Vec<u8>>;

impl Invocation<'_> {
    /// Starts this invocation beside the test, in `group`.
    pub fn spawn(self, group: ProcessGroup) -> Spawned {
        Spawned::start(self.command(), group)
    }
}

impl Spawned {
    /// Starts `command` beside the test, in `group`: a `bead` with an environment of its own, a
    /// shell, or a tool such as `br`. Its standard input is empty and its output is drained.
    pub fn start(mut command: Command, group: ProcessGroup) -> Spawned {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if group == ProcessGroup::Own {
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("cannot start {:?}: {error}", command.get_program()));
        let stdout = drain(child.stdout.take().expect("the standard output"));
        let stderr = drain(child.stderr.take().expect("the standard error"));
        Spawned {
            child,
            group,
            output: Some((stdout, stderr)),
        }
    }

    /// How it ended, or `None` while it runs.
    pub fn poll(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().expect("bead's status")
    }

    /// Sends SIGKILL: to its process group when it has one of its own, else to bead alone.
    pub fn kill(&self) -> rustix::io::Result<()> {
        let pid = Pid::from_child(&self.child);
        match self.group {
            ProcessGroup::Own => kill_process_group(pid, Signal::KILL),
            ProcessGroup::Inherited => kill_process(pid, Signal::KILL),
        }
    }

    /// How it ended and what it printed, or `None` if it has not ended within `limit`.
    pub fn within(&mut self, limit: Duration) -> Option<Run> {
        let deadline = Instant::now() + limit;
        let status = loop {
            if let Some(status) = self.poll() {
                break status;
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let (stdout, stderr) = self.output.take().expect("bead's output, read once");
        // a pipe a child of bead's still holds open ends soon after bead, or never
        let left = || deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(5);
        Some(Run {
            status,
            stdout: received(&stdout, left(), "standard output"),
            stderr: received(&stderr, left(), "standard error"),
        })
    }

    /// How it ended and what it printed; not ending within `limit` fails the test.
    pub fn communicate(&mut self, limit: Duration) -> Run {
        let run = self.within(limit);
        run.unwrap_or_else(|| panic!("bead had not ended after {limit:?}"))
    }

    /// Waits up to `limit` for `marker`, which a hook, a fake `git` or a failpoint makes. bead
    /// ending first fails the test with what it printed, and so does a marker that never comes.
    pub fn wait_for(&mut self, marker: &Path, limit: Duration) {
        let deadline = Instant::now() + limit;
        while !marker.exists() {
            if self.poll().is_some() {
                let printed = self.communicate(Duration::from_secs(10));
                panic!("bead ended before {}: {printed:?}", marker.display());
            }
            assert!(
                Instant::now() < deadline,
                "{} never came within {limit:?}",
                marker.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.kill();
            let _ = self.child.wait();
        }
    }
}

/// Starts a thread that reads `pipe` to its end: the channel it hands what it read to.
pub fn drain(mut pipe: impl Read + Send + 'static) -> Drained {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
}

/// What bead printed on `stream` once the pipe ended, as text: a pipe still open after `limit`,
/// or output that is not UTF-8, fails the test.
fn received(receiver: &Drained, limit: Duration, stream: &str) -> String {
    let bytes = receiver.recv_timeout(limit);
    let bytes = bytes.unwrap_or_else(|_| panic!("bead's {stream} had not ended"));
    String::from_utf8(bytes).unwrap_or_else(|error| panic!("bead's {stream} is not UTF-8: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Machine, Sandbox, executable};

    /// A machine whose `bead` is a shell script: `print` prints and exits 3, `mark PATH` makes
    /// PATH and then waits, `wait` only waits.
    fn stand_in() -> (Sandbox, Machine) {
        let this = std::env::current_exe().expect("the running test's own executable");
        let sandbox = Sandbox::selecting(this, None);
        let script = sandbox.root().join("bead");
        executable(
            &script,
            "#!/bin/sh\ncase \"$1\" in\n  print) echo out; echo err >&2; exit 3 ;;\n  \
             mark) touch \"$2\"; exec sleep 60 ;;\n  *) exec sleep 60 ;;\nesac\n",
        );
        let machine = Machine::new(&sandbox, "m").with_bead(script);
        (sandbox, machine)
    }

    #[test]
    fn what_a_spawned_bead_printed_comes_back_once_it_ends() {
        let (_sandbox, m) = stand_in();
        let run = m
            .cmd(["print"])
            .spawn(ProcessGroup::Inherited)
            .communicate(Duration::from_secs(10));
        assert_eq!(
            (run.returncode(), run.stdout.as_str(), run.stderr.as_str()),
            (3, "out\n", "err\n")
        );
    }

    #[test]
    fn a_bead_still_running_is_not_ended_within_its_limit_and_a_kill_ends_it() {
        let (_sandbox, m) = stand_in();
        let mut waiting = m.cmd(["wait"]).spawn(ProcessGroup::Own);
        assert!(waiting.within(Duration::from_millis(100)).is_none());
        waiting.kill().expect("its process group");
        assert_eq!(
            waiting.communicate(Duration::from_secs(10)).returncode(),
            -9
        );
    }

    #[test]
    fn a_marker_bead_makes_is_waited_for() {
        let (sandbox, m) = stand_in();
        let marker = sandbox.root().join("marker");
        let marker_arg = marker.to_str().expect("a UTF-8 path");
        let mut marking = m.cmd(["mark", marker_arg]).spawn(ProcessGroup::Own);
        marking.wait_for(&marker, Duration::from_secs(10));
    }

    #[test]
    #[should_panic(expected = "bead ended before")]
    fn a_bead_that_ends_before_its_marker_fails_the_test_with_what_it_printed() {
        let (sandbox, m) = stand_in();
        let mut printing = m.cmd(["print"]).spawn(ProcessGroup::Inherited);
        printing.wait_for(&sandbox.root().join("never"), Duration::from_secs(10));
    }
}
