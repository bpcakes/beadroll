//! What can go wrong on this machine's side, each with the code bead reports it under.

use beadroll_core::code::Code;
use beadroll_git::IoError;

/// A failure of the store, of workspace discovery or of the git that discovery runs.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A setting or the tracked configuration cannot be used (`CONFIG`).
    #[error("{0}")]
    Config(String),
    /// A file of the store, or one named on the command line, could not be read or written (`IO`).
    #[error("{0}")]
    Io(IoError),
    /// This machine's identity file is not one (`IDENTITY`).
    #[error("{0}")]
    Identity(String),
    /// The checkout belongs to no workspace bead can name (`NO_WORKSPACE`).
    #[error("{0}")]
    NoWorkspace(String),
    /// Text that is not valid UTF-8 where bead needs text (`VALIDATION`).
    #[error("{0}")]
    Validation(String),
    /// git failed, did not answer, or could not be started.
    #[error("{0}")]
    Git(#[from] beadroll_git::Error),
}

impl Error {
    /// The code bead reports this under.
    pub fn code(&self) -> Code {
        match self {
            Error::Config(_) => Code::Config,
            Error::Io(_) => Code::Io,
            Error::Identity(_) => Code::Identity,
            Error::NoWorkspace(_) => Code::NoWorkspace,
            Error::Validation(_) => Code::Validation,
            Error::Git(error) => git_code(error),
        }
    }

    /// An IO error with bead's own message rather than the system's.
    pub fn io_message(message: impl Into<String>) -> Error {
        Error::Io(IoError {
            errno: None,
            strerror: message.into(),
            path: None,
        })
    }
}

/// The code a git failure is reported under: a timeout is `REMOTE_UNREACHABLE`, a failed git
/// `GIT_ERROR`, a git that could not be started `IO`.
pub fn git_code(error: &beadroll_git::Error) -> Code {
    match error {
        beadroll_git::Error::Timeout { .. } => Code::RemoteUnreachable,
        beadroll_git::Error::Failed { .. } => Code::GitError,
        beadroll_git::Error::Io(_) => Code::Io,
        beadroll_git::Error::ConfigCount(_) => Code::Config,
    }
}

impl From<IoError> for Error {
    fn from(error: IoError) -> Error {
        Error::Io(error)
    }
}
