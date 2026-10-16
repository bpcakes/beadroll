//! Git execution failures and operating-system error formatting.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// An IO error formatted as `{strerror}: {path}` when a path is present, otherwise `[Errno N]
/// {strerror}`. Omits the standard library's `(os error N)` suffix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoError {
    /// The error number, when the error came from the operating system.
    pub errno: Option<i32>,
    /// The system's text for it (`strerror`), or the whole message of an error with no number.
    pub strerror: String,
    /// The affected path; for rename errors, the destination.
    pub path: Option<PathBuf>,
}

impl IoError {
    /// `error`, concerning `path`.
    pub fn at(error: &io::Error, path: impl AsRef<Path>) -> IoError {
        IoError {
            path: Some(path.as_ref().to_path_buf()),
            ..IoError::bare(error)
        }
    }

    /// `error`, concerning no path in particular (a write to a file already open, say).
    pub fn bare(error: &io::Error) -> IoError {
        IoError {
            errno: error.raw_os_error(),
            strerror: strerror(error),
            path: None,
        }
    }

    /// Whether the system said the path does not exist (`ENOENT`).
    pub fn not_found(&self) -> bool {
        self.errno == Some(libc::ENOENT)
    }
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.path, self.errno) {
            (Some(path), _) => write!(f, "{}: {}", self.strerror, path.display()),
            (None, Some(errno)) => write!(f, "[Errno {errno}] {}", self.strerror),
            (None, None) => f.write_str(&self.strerror),
        }
    }
}

impl std::error::Error for IoError {}

/// The system's strerror text without the standard library's `(os error N)` suffix.
pub fn strerror(error: &io::Error) -> String {
    let text = error.to_string();
    match error.raw_os_error() {
        Some(code) => text
            .strip_suffix(&format!(" (os error {code})"))
            .map_or_else(|| text.clone(), str::to_owned),
        None => text,
    }
}

/// Why running git did not give an answer bead can use.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// git did not finish within its time (code `REMOTE_UNREACHABLE`): `"git {first argument} gave no
    /// answer within {seconds}s"`, the time as it was set, to the millisecond (`45`, `0.2`).
    #[error("git {command} gave no answer within {seconds}s")]
    Timeout {
        /// git's first argument (`fetch`, `push`).
        command: String,
        /// The allowed duration in seconds, to the millisecond and without trailing zeros.
        seconds: String,
    },
    /// git failed (code `GIT_ERROR`): `"git {the first two arguments}: {what git said}"`, or
    /// `killed by SIGNAME` when a signal ended it.
    #[error("git {command}: {said}")]
    Failed {
        /// git's first two arguments, joined by a space.
        command: String,
        /// git's standard error, trimmed, after the signal that killed it if one did.
        said: String,
    },
    /// git could not be started, or its output could not be kept (code `IO`).
    #[error("{0}")]
    Io(IoError),
    /// The caller's GIT_CONFIG_COUNT cannot be parsed, so extra settings cannot safely be
    /// appended; reported as CONFIG.
    #[error("GIT_CONFIG_COUNT is {0:?}, not a number of configuration settings")]
    ConfigCount(String),
}
