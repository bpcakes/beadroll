//! One run of `bead`, as `Run.call` makes it: started with a 180-second limit, its output read to
//! the end, its answer (the JSON document, or the error code) taken apart.

use std::fmt;
use std::io::Read;
use std::os::unix::process::ExitStatusExt as _;
use std::process::{ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use beadroll_test_support::{Invocation, Json, parse_json};

/// How long a run may take before it counts as hung.
const HANG_AFTER: Duration = Duration::from_secs(180);

/// The exit code, negated signal number, or `HANG`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rc {
    Exit(i32),
    Hang,
}

impl Rc {
    /// Whether the run ended with this status.
    pub(super) fn is(self, status: i32) -> bool {
        self == Rc::Exit(status)
    }
}

impl fmt::Display for Rc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rc::Exit(status) => write!(f, "{status}"),
            Rc::Hang => f.write_str("HANG"),
        }
    }
}

/// One step of a schedule, as `Run.call` records it.
#[derive(Clone, Debug)]
pub(super) struct Step {
    /// Its label: `op`, `R`, `setup`, `view after op`, ...
    pub(super) step: String,
    pub(super) machine: String,
    pub(super) thread: String,
    pub(super) argv: Vec<String>,
    pub(super) rc: Rc,
    /// The error code a failed run reported.
    pub(super) code: Option<String>,
    pub(super) ms: u128,
    /// The JSON document a successful run printed.
    pub(super) out: Option<Json>,
    /// What it printed, whole.
    pub(super) raw: String,
    /// The end of what it said on standard error: its last 400 characters.
    pub(super) err: String,
}

impl Step {
    /// The first item of the list it printed, or an empty object: `(step["out"] or [{}])[0]`.
    pub(super) fn first(&self) -> Json {
        match &self.out {
            Some(out) if out.truthy() => out[0].clone(),
            _ => Json::Object(Default::default()),
        }
    }
}

/// What a finished (or abandoned) run printed.
pub(super) struct Ran {
    pub(super) rc: Rc,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) ms: u128,
}

impl Ran {
    /// Whether bead crashed rather than returning a structured result: a hang, traceback, panic
    /// (`panicked at` or exit 101), SIGABRT or SIGSEGV.
    pub(super) fn crashed(&self, code: Option<&str>) -> bool {
        self.rc == Rc::Hang
            || code == Some("TRACEBACK")
            || self.stderr.contains("panicked at")
            || matches!(self.rc, Rc::Exit(101 | -6 | -11))
    }
}

/// Runs an invocation for at most 180 seconds. A timeout kills it and records a hang with no
/// output. Output that is not UTF-8 fails the schedule.
pub(super) fn exec(invocation: Invocation<'_>) -> Ran {
    let started = Instant::now();
    let mut command = invocation.command();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("cannot start {:?}: {error}", command.get_program()));
    let stdout = drain(child.stdout.take().expect("bead's standard output"));
    let stderr = drain(child.stderr.take().expect("bead's standard error"));
    let deadline = started + HANG_AFTER;
    // As `communicate` does: both pipes read to their end, then the exit awaited.
    let finished = received(&stdout, deadline).and_then(|out| {
        let err = received(&stderr, deadline)?;
        let status = exited(&mut child, deadline)?;
        Some((status, out, err))
    });
    let ms = started.elapsed().as_millis();
    let Some((status, out, err)) = finished else {
        let _ = child.kill();
        let _ = child.wait();
        return Ran {
            rc: Rc::Hang,
            stdout: String::new(),
            stderr: "no exit within 180 s".to_owned(),
            ms,
        };
    };
    Ran {
        rc: returncode(status),
        stdout: text(out, "standard output"),
        stderr: text(err, "standard error"),
        ms,
    }
}

/// A thread that reads `pipe` to its end, and the channel it hands what it read to.
fn drain(mut pipe: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
}

/// What `drain`'s thread read, if the pipe ended before `deadline`.
fn received(receiver: &Receiver<Vec<u8>>, deadline: Instant) -> Option<Vec<u8>> {
    let left = deadline.saturating_duration_since(Instant::now());
    receiver.recv_timeout(left).ok()
}

/// How `child` ended, if it did before `deadline`.
fn exited(child: &mut std::process::Child, deadline: Instant) -> Option<ExitStatus> {
    loop {
        if let Some(status) = child.try_wait().expect("bead's status") {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The exit code, or minus the number of the signal that ended the process.
fn returncode(status: ExitStatus) -> Rc {
    match (status.code(), status.signal()) {
        (Some(code), _) => Rc::Exit(code),
        (None, Some(signal)) => Rc::Exit(-signal),
        (None, None) => panic!("{status:?} has neither an exit code nor a signal"),
    }
}

/// Output decoded as `text=True` decodes it: UTF-8, or the schedule fails.
fn text(bytes: Vec<u8>, stream: &str) -> String {
    String::from_utf8(bytes).unwrap_or_else(|error| panic!("bead's {stream} is not UTF-8: {error}"))
}

/// The answer of a run: the document a successful run printed, and the code a failed one reported
/// (`TRACEBACK` when it printed a traceback instead).
pub(super) fn answer(ran: &Ran) -> (Option<Json>, Option<String>) {
    let out = if ran.rc.is(0) && !ran.stdout.trim().is_empty() {
        parse_json(&ran.stdout).ok()
    } else {
        None
    };
    let code = if ran.rc.is(0) || ran.rc == Rc::Hang || ran.stderr.trim().is_empty() {
        None
    } else {
        let reported = parse_json(&ran.stderr).ok();
        let code = reported.and_then(|error| error["error"]["code"].as_str().map(str::to_owned));
        code.or_else(|| {
            let traceback = ran.stderr.contains("Traceback");
            traceback.then(|| "TRACEBACK".to_owned())
        })
    };
    (out, code)
}

/// The last `n` Unicode scalar values of `text`.
pub(super) fn tail(text: &str, n: usize) -> &str {
    let skip = text.chars().count().saturating_sub(n);
    text.char_indices()
        .nth(skip)
        .map_or("", |(at, _)| &text[at..])
}
