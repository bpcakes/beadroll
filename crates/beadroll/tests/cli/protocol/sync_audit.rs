//! `SyncAudit`: the first independent audit (AUDIT, 2026-10-06) of the journal and its sync: an
//! append cut short, a journal forked by a rewritten or a copied history, rotation under parallel
//! writers, a remote that lost its history, lost acknowledgements, and git stalled in a push or a
//! fetch. Its tests are grouped by subject in the modules below; the helpers they share, the
//! class's own (`setUp`, `two`, `own_segment`, `remote_blob`, `wait_marker`, `git_wrapper`) among
//! them, are here. The kill in the middle of an append and the exhaustion of a command's retries
//! are `rust_only`: they stop the Rust `bead` at its `append.mid` failpoint, and skip its waits
//! between rounds at `backoff`.

use std::ffi::{OsStr, OsString};
use std::fs::Permissions;
use std::io::Read;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use beadroll_test_support::{Invocation, Machine, Run, Sandbox, git};
use rustix::process::{Pid, Signal, kill_process, kill_process_group};

mod forks;
mod lost_acks;
mod remote_ref;
mod segments;
mod stalls;

/// How a test makes its machines: `Machine::new`, or `crate::rust_only` for a test that injects its
/// fault through a failpoint.
type Make = fn(&Sandbox, &str) -> Machine;

/// The class's `setUp`: machine a, its tracker initialised with the prefix `audit`, and the key of
/// the one issue it holds, `Original issue`.
fn setup(sandbox: &Sandbox) -> (Machine, String) {
    setup_with(sandbox, Machine::new)
}

/// `setUp` with machine a made by `make`.
fn setup_with(sandbox: &Sandbox, make: Make) -> (Machine, String) {
    let a = make(sandbox, "a");
    a.bead(["init", "--prefix", "audit"]);
    let created = a.bead(["create", "Original issue"]).expect("the issue");
    let key = created["id"].as_str().expect("the issue's id").to_owned();
    (a, key)
}

/// `two`: a publishes its tracker, and a second machine, b, syncs it.
fn two(sandbox: &Sandbox, a: &Machine) -> Machine {
    two_with(sandbox, a, Machine::new)
}

/// `two` with machine b made by `make`.
fn two_with(sandbox: &Sandbox, a: &Machine, make: Make) -> Machine {
    a.bead(["sync"]);
    let b = make(sandbox, "b");
    b.bead(["sync"]);
    b
}

/// `own_segment`: the last segment of `machine`'s own journal, the one its next event is appended
/// to, `events/<machine>/*.jsonl` in its store by name.
fn own_segment(machine: &Machine) -> PathBuf {
    let info = machine.bead(["info"]).expect("bead info prints a record");
    let own = info["machine"].as_str().expect("the machine's id");
    let mut found = jsonl(&machine.store().join("events").join(own));
    found.sort();
    found.pop().expect("a segment of the machine's own journal")
}

/// The entries of `dir` whose names end with `.jsonl`, in directory order, as
/// `Path.glob("*.jsonl")` lists them.
fn jsonl(dir: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(dir).expect("a journal's folder");
    entries
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| {
            let name = path.file_name().expect("a name");
            name.as_encoded_bytes().ends_with(b".jsonl")
        })
        .collect()
}

/// `support.git("--git-dir", origin, ARGS)`: what git ARGS printed, run on the sandbox's remote.
fn remote_git(sandbox: &Sandbox, args: &[&str]) -> String {
    let words = [OsStr::new("--git-dir"), sandbox.origin().as_os_str()];
    git(words.into_iter().chain(args.iter().map(OsStr::new)), None)
}

/// `remote_blob`: the file `path` of the journal as the remote's `refs/beads/journal` holds it.
fn remote_blob(sandbox: &Sandbox, path: &str) -> String {
    remote_git(sandbox, &["show", &format!("refs/beads/journal:{path}")])
}

/// `wait_marker`: waits up to 15 seconds for the instrumented git to leave `marker`. The worker
/// ending first fails the test with what it printed, and so does a marker that never comes.
fn wait_marker(marker: &Path, worker: &mut Worker) {
    let until = Instant::now() + Duration::from_secs(15);
    while !marker.exists() && Instant::now() < until {
        if worker.poll().is_some() {
            let printed = worker.communicate(None);
            panic!("worker exited before marker: {printed:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "instrumented fault point was not reached");
}

/// `git_wrapper`: a `PATH` whose first `git` holds up the transport operation `action` (`push` or
/// `fetch`) for 20 seconds, and runs every other git command unchanged. With `after`, it performs
/// the operation first, then leaves `marker` and waits before it answers with git's status: the
/// remote has the push, bead has not heard. Without, it leaves `marker` and waits first. The
/// Python wrapper is a Python script that sleeps in its own process; this one is a shell script
/// whose `sleep` is a child, left to run out its 20 seconds when bead kills git's client alone, as
/// it does when a push times out.
fn git_wrapper(sandbox: &Sandbox, action: &str, marker: &Path, after: bool) -> OsString {
    let folder = sandbox.root().join(format!("wrapper-{action}"));
    std::fs::create_dir_all(&folder).unwrap();
    let (git, marker) = (quoted(&real_git()), quoted(marker));
    let held = if after {
        format!(
            "  {git} \"$@\"\n  status=$?\n  printf ready > {marker}\n  sleep 20\n  exit $status\n"
        )
    } else {
        format!("  printf ready > {marker}\n  sleep 20\n")
    };
    // Only delay the specified transport operation. All other git commands remain real.
    let script =
        format!("#!/bin/sh\nif [ \"$1\" = {action} ]; then\n{held}fi\nexec {git} \"$@\"\n");
    let wrapper = folder.join("git");
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, Permissions::from_mode(0o755)).unwrap();
    let mut path = folder.into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").expect("the test's PATH"));
    path
}

/// The git the test's `PATH` finds, as `shutil.which("git")` finds it: the one the wrapper runs.
fn real_git() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let executable = |file: &Path| {
        let mode = std::fs::metadata(file).map(|meta| (meta.is_file(), meta.permissions().mode()));
        mode.is_ok_and(|(is_file, mode)| is_file && mode & 0o111 != 0)
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|git| executable(git))
        .expect("git on PATH")
}

/// `path` quoted for the shell.
fn quoted(path: &Path) -> String {
    let text = path.to_str().expect("a UTF-8 path");
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// A `bead` running beside the test, as `subprocess.Popen` starts one: nothing on its standard
/// input, its output piped and read to the end on threads of their own, so that it never waits for
/// the test to read. One started `alone` leads a process group of its own, which a kill takes
/// whole: what `start_new_session=True` gives the Python test for `os.killpg`, without the new
/// session, which only code between fork and exec could ask for. A worker dropped while it runs is
/// killed and reaped: the Python tests' `finally` blocks.
struct Worker {
    child: Child,
    alone: bool,
    output: Option<(Drained, Drained)>,
}

/// What a pipe held, handed over by the thread that read it to its end.
type Drained = Receiver<Vec<u8>>;

impl Worker {
    /// Starts `invocation`.
    fn start(invocation: Invocation<'_>, alone: bool) -> Worker {
        let mut command = invocation.command();
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if alone {
            command.process_group(0);
        }
        let mut child = command.spawn().expect("bead starts");
        let stdout = drain(child.stdout.take().expect("bead's standard output"));
        let stderr = drain(child.stderr.take().expect("bead's standard error"));
        Worker {
            child,
            alone,
            output: Some((stdout, stderr)),
        }
    }

    /// `poll()`: how it ended, or `None` while it runs.
    fn poll(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().expect("the worker's status")
    }

    /// SIGKILL to its process group when it was started alone (`os.killpg`), else to it (`kill()`).
    fn kill(&self) -> rustix::io::Result<()> {
        let pid = Pid::from_child(&self.child);
        if self.alone {
            kill_process_group(pid, Signal::KILL)
        } else {
            kill_process(pid, Signal::KILL)
        }
    }

    /// `communicate(timeout)`: waits for it to end and for the end of its output, and returns what
    /// it printed. Not done within `timeout`, the test fails, as `TimeoutExpired` fails the Python
    /// test; without one, it waits as long as that takes.
    fn communicate(&mut self, timeout: Option<Duration>) -> Run {
        let until = timeout.map(|timeout| Instant::now() + timeout);
        let status = loop {
            if let Some(status) = self.poll() {
                break status;
            }
            let expired = until.is_some_and(|until| Instant::now() >= until);
            assert!(!expired, "bead had not ended after {timeout:?}");
            std::thread::sleep(Duration::from_millis(10));
        };
        let (stdout, stderr) = self.output.take().expect("output not yet read");
        let left = || until.map(|until| until.saturating_duration_since(Instant::now()));
        let stdout = received(&stdout, left());
        let stderr = received(&stderr, left());
        Run {
            status,
            stdout: text(stdout, "standard output"),
            stderr: text(stderr, "standard error"),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.kill();
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

/// What `drain`'s thread read, once the pipe has ended: within `timeout`, or the test fails.
fn received(receiver: &Drained, timeout: Option<Duration>) -> Vec<u8> {
    let bytes = match timeout {
        Some(timeout) => receiver.recv_timeout(timeout).ok(),
        None => receiver.recv().ok(),
    };
    bytes.unwrap_or_else(|| panic!("bead's output had not ended after {timeout:?}"))
}

/// Output decoded as `text=True` decodes it: UTF-8, or the test fails.
fn text(bytes: Vec<u8>, stream: &str) -> String {
    String::from_utf8(bytes).unwrap_or_else(|error| panic!("bead's {stream} is not UTF-8: {error}"))
}
