//! What can go wrong while writing, syncing or settling, each with the code bead reports it under.

use beadroll_core::code::Code;
use beadroll_core::json::Value;
use beadroll_core::state::KeySpace;
use beadroll_store::ViewError;

/// A failure of a local write or of the exchange with the coordination remote.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum Error {
    /// The store, workspace discovery or a git it runs failed.
    #[error("{0}")]
    Store(#[from] beadroll_store::Error),
    /// A renamed copy found every free key taken (`KEY_SPACE`).
    #[error("{0}")]
    KeySpace(#[from] KeySpace),
    /// An event this bead would write breaks the rules replay applies (`VALIDATION`).
    #[error("{0}")]
    Validation(String),
    /// There is no tracker here and none on the remote (`NOT_INITIALIZED`).
    #[error("{0}")]
    NotInitialized(String),
    /// This machine's journal no longer forms one chain, or differs from the published one: its
    /// identity is in use somewhere else (`JOURNAL_FORK`).
    #[error("{0}")]
    JournalFork(String),
    /// The remote holds another attempt under the id of one this machine has pending
    /// (`ATTEMPT_FORK`).
    #[error("{0}")]
    AttemptFork(String),
    /// The published tracker has no readable manifest (`MANIFEST_MISSING`).
    #[error("{0}")]
    ManifestMissing(String),
    /// The published tracker belongs to another workspace (`WORKSPACE_MISMATCH`); `found` is the id
    /// its manifest names, whatever it is.
    #[error("{message}")]
    WorkspaceMismatch {
        /// The message.
        message: String,
        /// The manifest's `workspace`.
        found: Value,
    },
    /// The published tracker is in a format this bead does not read (`FORMAT_UNSUPPORTED`).
    #[error("{0}")]
    FormatUnsupported(String),
    /// The remote names objects it no longer has (`REMOTE_DAMAGED`).
    #[error("{0}")]
    RemoteDamaged(String),
    /// The remote did not answer, or the push did not complete (`REMOTE_UNREACHABLE`).
    #[error("{0}")]
    RemoteUnreachable(String),
    /// The remote refuses bead's update of its ref (`REMOTE_REFUSED`).
    #[error("{0}")]
    RemoteRefused(String),
    /// This machine's sync repository is damaged (`SYNC_REPO_DAMAGED`).
    #[error("{0}")]
    SyncRepoDamaged(String),
    /// The remote journal kept moving (`SYNC_CONFLICT`).
    #[error("{0}")]
    SyncConflict(String),
    /// `git init` of the sync repository failed (`GIT_ERROR`).
    #[error("{0}")]
    GitError(String),
}

impl Error {
    /// The code bead reports this under.
    pub fn code(&self) -> Code {
        match self {
            Error::Store(error) => error.code(),
            Error::KeySpace(error) => error.code(),
            Error::Validation(_) => Code::Validation,
            Error::NotInitialized(_) => Code::NotInitialized,
            Error::JournalFork(_) => Code::JournalFork,
            Error::AttemptFork(_) => Code::AttemptFork,
            Error::ManifestMissing(_) => Code::ManifestMissing,
            Error::WorkspaceMismatch { .. } => Code::WorkspaceMismatch,
            Error::FormatUnsupported(_) => Code::FormatUnsupported,
            Error::RemoteDamaged(_) => Code::RemoteDamaged,
            Error::RemoteUnreachable(_) => Code::RemoteUnreachable,
            Error::RemoteRefused(_) => Code::RemoteRefused,
            Error::SyncRepoDamaged(_) => Code::SyncRepoDamaged,
            Error::SyncConflict(_) => Code::SyncConflict,
            Error::GitError(_) => Code::GitError,
        }
    }

    /// Whether this came from the operating system rather than from bead (Python's `OSError`, as
    /// opposed to a `BeadError`), and what the system said: `publish_soon` words the two apart.
    pub fn os_error(&self) -> Option<&str> {
        let io = match self {
            Error::Store(beadroll_store::Error::Io(io))
            | Error::Store(beadroll_store::Error::Git(beadroll_git::Error::Io(io))) => io,
            _ => return None,
        };
        io.errno.map(|_| io.strerror.as_str())
    }
}

impl From<beadroll_git::Error> for Error {
    fn from(error: beadroll_git::Error) -> Error {
        Error::Store(beadroll_store::Error::Git(error))
    }
}

impl From<beadroll_git::IoError> for Error {
    fn from(error: beadroll_git::IoError) -> Error {
        Error::Store(beadroll_store::Error::Io(error))
    }
}

impl From<ViewError> for Error {
    fn from(error: ViewError) -> Error {
        match error {
            ViewError::Store(error) => Error::Store(error),
            ViewError::KeySpace(error) => Error::KeySpace(error),
        }
    }
}
