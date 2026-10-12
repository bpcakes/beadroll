//! Fault injection compiled into every build and configured once through BEAD_FAILPOINTS
//! (`docs/RUST-PORT.md` §4.5). Semicolon-separated specs are `point=action`,
//! `point@nth(k)=action` or `point@always=action`. Malformed specs, unknown points, unsupported
//! actions and duplicate points return CONFIG, preventing silent test passes. Crash hooks
//! BEAD_TEST_CRASH_BEFORE_PUSH and BEAD_TEST_CRASH_AFTER_PUSH select
//! ownership.before-publish=exit(98) and ownership.after-publish-ok=exit(99).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use beadroll_core::code::Code;

use crate::error::Error;

/// The points bead has, besides `atomic-write:<path part>` and `atomic-written:<path part>`.
pub const POINTS: [&str; 19] = [
    "ownership.before-publish",
    "ownership.after-publish-ok",
    "ownership.after-confirm",
    "push.before",
    "push.after-ok",
    "push.reply",
    "publish.result",
    "publish.before",
    "fetch.result",
    "fetch.retry",
    "append.mid",
    "backoff",
    "claim.before-push",
    "pending.after-list",
    "journal.after-enumerate",
    "projection.texts-appended",
    "view.before-load-pending",
    "identity.before-link",
    "clock",
];

/// The longest a `pause-until` waits for its `.go` file before it lets the step go on.
const PAUSE_LIMIT: Duration = Duration::from_secs(300);

/// Which hits of a point fire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selector {
    /// The first.
    First,
    /// The `k`-th, counting from one.
    Nth(u64),
    /// Every one.
    Always,
}

/// What a failpoint does when it fires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Exits immediately with this status, without unwinding.
    Exit(i32),
    /// A real `SIGKILL`.
    Sigkill,
    /// At `append.mid`: half the line written and fsynced, then `SIGKILL`.
    TornSigkill,
    /// At `atomic-write`: the write fails with this error number.
    Errno(i32),
    /// At `push.reply`: a push the remote accepted is reported as failed, its status line dropped.
    FailReply,
    /// At `publish.result`: the publish fails with this code; at `ownership.after-confirm`, the
    /// command does, its attempt confirmed and its pending twin still there.
    Error(Code),
    /// Touch `PATH.ready`, then wait for `PATH.go`.
    PauseUntil(PathBuf),
    /// Sleep this many milliseconds.
    Sleep(u64),
    /// At `backoff`: skip the wait.
    Skip,
    /// At `clock`: shift the clock by this many nanoseconds.
    Offset(i128),
    /// At `clock`: the clock reads this many nanoseconds since the epoch.
    At(i128),
}

/// What a fired failpoint leaves for the code at the point to do: the actions that act by
/// themselves (`exit`, `sigkill`, `sleep`, `pause-until`) have acted by then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fired {
    /// Write half the line, fsync it, then [`kill_self`].
    TornSigkill,
    /// Fail the write with this error number.
    Errno(i32),
    /// Report the accepted push as failed.
    FailReply,
    /// Fail with this code.
    Error(Code),
    /// Skip the step.
    Skip,
    /// Shift the clock by this many nanoseconds.
    Offset(i128),
    /// The clock reads this many nanoseconds since the epoch.
    At(i128),
}

/// One spec.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Spec {
    point: String,
    selector: Selector,
    action: Action,
}

/// The failpoints of one invocation, with how often each point has been hit.
#[derive(Debug, Default)]
pub struct Failpoints {
    specs: Vec<Spec>,
    hits: Mutex<BTreeMap<String, u64>>,
}

impl Failpoints {
    /// None: every point passes.
    pub fn none() -> Failpoints {
        Failpoints::default()
    }

    /// The failpoints of `spec` (`BEAD_FAILPOINTS`) and of the crash hooks set (non-empty)
    /// `BEAD_TEST_CRASH_BEFORE_PUSH` and `BEAD_TEST_CRASH_AFTER_PUSH`.
    pub fn from_env(
        spec: Option<&OsStr>,
        crash_before: bool,
        crash_after: bool,
    ) -> Result<Failpoints, Error> {
        let text = match spec {
            Some(spec) => spec
                .to_str()
                .ok_or_else(|| Error::Config("BEAD_FAILPOINTS must be text (UTF-8)".to_owned()))?,
            None => "",
        };
        let mut specs =
            parse(text).map_err(|why| Error::Config(format!("BEAD_FAILPOINTS: {why}")))?;
        for (set, point, status, name) in [
            (
                crash_before,
                "ownership.before-publish",
                98,
                "BEAD_TEST_CRASH_BEFORE_PUSH",
            ),
            (
                crash_after,
                "ownership.after-publish-ok",
                99,
                "BEAD_TEST_CRASH_AFTER_PUSH",
            ),
        ] {
            if !set {
                continue;
            }
            if specs.iter().any(|s| s.point == point) {
                return Err(Error::Config(format!(
                    "{name} and BEAD_FAILPOINTS both name {point}"
                )));
            }
            specs.push(Spec {
                point: point.to_owned(),
                selector: Selector::First,
                action: Action::Exit(status),
            });
        }
        Ok(Failpoints {
            specs,
            hits: Mutex::new(BTreeMap::new()),
        })
    }

    /// Whether a spec names `point`: code that must prepare for one (a half-written line) asks
    /// first, and does nothing different otherwise.
    pub fn has(&self, point: &str) -> bool {
        self.specs.iter().any(|s| s.point == point)
    }

    /// How often `point`, as a spec names it, has been passed in this invocation: a test checks
    /// that the fault it injected was reached. A point no spec names is not counted.
    pub fn hits(&self, point: &str) -> u64 {
        let hits = self
            .hits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        hits.get(point).copied().unwrap_or(0)
    }

    /// Passes `point`: counts the hit, and when its spec fires acts (`exit`, `sigkill`, `sleep`,
    /// `pause-until`), or returns what the caller is to do.
    pub fn hit(&self, point: &str) -> Option<Fired> {
        let index = self.specs.iter().position(|s| s.point == point)?;
        self.fire(index)
    }

    /// Passes the atomic write of `target` (`kind` is `atomic-write`, before the rename, or
    /// `atomic-written`, after it): the first spec whose path part `target` holds.
    pub fn hit_path(&self, kind: &str, target: &Path) -> Option<Fired> {
        let shown = target.to_string_lossy();
        let index = self.specs.iter().position(|s| {
            s.point
                .strip_prefix(kind)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|part| shown.contains(part))
        })?;
        self.fire(index)
    }

    fn fire(&self, index: usize) -> Option<Fired> {
        let spec = self.specs.get(index)?;
        let count = {
            let mut hits = self
                .hits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let count = hits.entry(spec.point.clone()).or_insert(0);
            *count += 1;
            *count
        };
        let fires = match spec.selector {
            Selector::First => count == 1,
            Selector::Nth(k) => count == k,
            Selector::Always => true,
        };
        if !fires {
            return None;
        }
        match &spec.action {
            Action::Exit(status) => std::process::exit(*status),
            Action::Sigkill => kill_self(),
            Action::Sleep(ms) => {
                std::thread::sleep(Duration::from_millis(*ms));
                None
            }
            Action::PauseUntil(path) => {
                pause_until(path);
                None
            }
            Action::TornSigkill => Some(Fired::TornSigkill),
            Action::Errno(errno) => Some(Fired::Errno(*errno)),
            Action::FailReply => Some(Fired::FailReply),
            Action::Error(code) => Some(Fired::Error(*code)),
            Action::Skip => Some(Fired::Skip),
            Action::Offset(ns) => Some(Fired::Offset(*ns)),
            Action::At(ns) => Some(Fired::At(*ns)),
        }
    }
}

/// A real `SIGKILL` of this process: nothing after it runs.
pub fn kill_self() -> ! {
    let _ = rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::KILL);
    // the signal cannot be caught; should it not be delivered, the process still ends here
    std::process::exit(137)
}

/// Touches `path.ready`, then waits for `path.go` (at most [`PAUSE_LIMIT`]).
fn pause_until(path: &Path) {
    let with = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let _ = std::fs::write(with(".ready"), b"");
    let go = with(".go");
    let started = Instant::now();
    while !go.exists() && started.elapsed() < PAUSE_LIMIT {
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The specs of `text`, checked.
fn parse(text: &str) -> Result<Vec<Spec>, String> {
    let mut specs: Vec<Spec> = Vec::new();
    for item in text
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (head, action) = item
            .split_once('=')
            .ok_or_else(|| format!("{item:?} is not point=action"))?;
        let (point, selector) = match head.split_once('@') {
            Some((point, selector)) => (point, read_selector(selector)?),
            None => (head, Selector::First),
        };
        let point = point.trim();
        known(point)?;
        let action = read_action(action.trim())?;
        allowed(point, &action)?;
        if specs.iter().any(|s| s.point == point) {
            return Err(format!("{point} is given twice"));
        }
        specs.push(Spec {
            point: point.to_owned(),
            selector,
            action,
        });
    }
    Ok(specs)
}

fn known(point: &str) -> Result<(), String> {
    let path_point = ["atomic-write:", "atomic-written:"].iter().any(|kind| {
        point
            .strip_prefix(kind)
            .is_some_and(|part| !part.is_empty())
    });
    if path_point || POINTS.contains(&point) {
        return Ok(());
    }
    Err(format!("there is no point {point:?}"))
}

/// Whether `point` can take `action`: every point can exit, be killed, sleep or pause; the rest
/// belong to the points named in `docs/RUST-PORT.md` §4.5.
fn allowed(point: &str, action: &Action) -> Result<(), String> {
    let fits = match action {
        Action::Exit(_) | Action::Sigkill | Action::Sleep(_) | Action::PauseUntil(_) => {
            point != "clock"
        }
        Action::TornSigkill => point == "append.mid",
        Action::Errno(_) => point.starts_with("atomic-write:"),
        Action::FailReply => point == "push.reply",
        Action::Error(_) => matches!(
            point,
            "publish.result" | "fetch.result" | "ownership.after-confirm"
        ),
        Action::Skip => matches!(point, "backoff" | "fetch.retry"),
        Action::Offset(_) | Action::At(_) => point == "clock",
    };
    if fits {
        Ok(())
    } else {
        Err(format!("{point} cannot take {action:?}"))
    }
}

fn read_selector(text: &str) -> Result<Selector, String> {
    if text == "always" {
        return Ok(Selector::Always);
    }
    let k = text
        .strip_prefix("nth(")
        .and_then(|rest| rest.strip_suffix(')'))
        .and_then(|k| k.parse::<u64>().ok())
        .filter(|k| *k >= 1)
        .ok_or_else(|| format!("{text:?} is not a selector: always, or nth(k) with k from 1"))?;
    Ok(Selector::Nth(k))
}

fn read_action(text: &str) -> Result<Action, String> {
    let (name, argument) = match text.split_once('(') {
        Some((name, rest)) => (
            name,
            Some(
                rest.strip_suffix(')')
                    .ok_or_else(|| format!("{text:?} does not close its parenthesis"))?,
            ),
        ),
        None => (text, None),
    };
    let wrong = || format!("{text:?} is not an action");
    let action = match (name, argument) {
        ("exit", Some(n)) => Action::Exit(n.parse().map_err(|_| wrong())?),
        ("sigkill", None) => Action::Sigkill,
        ("torn-sigkill", None) => Action::TornSigkill,
        ("errno", Some(name)) => Action::Errno(errno_named(name).ok_or_else(wrong)?),
        ("fail-reply", None) => Action::FailReply,
        ("error", Some(code)) => Action::Error(
            Code::ALL
                .into_iter()
                .find(|c| c.as_str() == code)
                .ok_or_else(wrong)?,
        ),
        ("pause-until", Some(path)) if !path.is_empty() => Action::PauseUntil(PathBuf::from(path)),
        ("sleep", Some(ms)) => Action::Sleep(ms.parse().map_err(|_| wrong())?),
        ("skip", None) => Action::Skip,
        ("offset", Some(duration)) => Action::Offset(nanoseconds(duration).ok_or_else(wrong)?),
        ("at", Some(time)) => Action::At(time.parse().map_err(|_| wrong())?),
        _ => return Err(wrong()),
    };
    Ok(action)
}

/// A signed duration with its unit (`-48h`, `+1d`, `250ms`) in nanoseconds.
fn nanoseconds(text: &str) -> Option<i128> {
    let digits_end = text
        .char_indices()
        .find(|(at, c)| !(c.is_ascii_digit() || (*at == 0 && matches!(c, '+' | '-'))))
        .map_or(text.len(), |(at, _)| at);
    let (number, unit) = text.split_at(digits_end);
    let number: i128 = number.parse().ok()?;
    let scale: i128 = match unit {
        "ns" => 1,
        "us" => 1_000,
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "m" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        "d" => 86_400_000_000_000,
        _ => return None,
    };
    number.checked_mul(scale)
}

/// The error numbers a failpoint may fail a write with, by name.
fn errno_named(name: &str) -> Option<i32> {
    use rustix::io::Errno;
    let errno = match name {
        "ENOSPC" => Errno::NOSPC,
        "EFBIG" => Errno::FBIG,
        "EIO" => Errno::IO,
        "EACCES" => Errno::ACCESS,
        "EPERM" => Errno::PERM,
        "EROFS" => Errno::ROFS,
        "EDQUOT" => Errno::DQUOT,
        "EMFILE" => Errno::MFILE,
        "ENFILE" => Errno::NFILE,
        "ENOENT" => Errno::NOENT,
        "EINTR" => Errno::INTR,
        _ => return None,
    };
    Some(errno.raw_os_error())
}

#[cfg(test)]
mod tests;
