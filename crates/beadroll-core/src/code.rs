//! The codes bead's errors carry on the wire (`{"error": {"code", "message"}}`), which `bead prime`
//! lists for agents: a closed set, so a code no agent knows cannot appear.

use std::fmt;

/// An error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Code {
    /// Someone else holds the issue.
    AlreadyClaimed,
    /// A pending attempt differs from the remote's copy: the machine identity is in use twice.
    AttemptFork,
    /// The issue is claimed by another actor.
    ClaimedByOther,
    /// A setting or tracked configuration is unusable.
    Config,
    /// The change would close a dependency cycle.
    Cycle,
    /// The published journal is in a format this bead does not read.
    FormatUnsupported,
    /// git failed.
    GitError,
    /// This machine's identity is unusable.
    Identity,
    /// A file could not be read or written.
    Io,
    /// No issue has that key.
    IssueNotFound,
    /// This machine's journal no longer forms one chain.
    JournalFork,
    /// No free key is left to mint or rename to.
    KeySpace,
    /// The published journal has no readable manifest.
    ManifestMissing,
    /// No tracker has been made for this workspace.
    NotInitialized,
    /// Only the owner may do that.
    NotOwner,
    /// No coordination remote is configured.
    NoRemote,
    /// This checkout belongs to no workspace.
    NoWorkspace,
    /// The remote names objects it no longer has.
    RemoteDamaged,
    /// The remote refused the push.
    RemoteRefused,
    /// The remote did not answer.
    RemoteUnreachable,
    /// The issue's status is disputed.
    StatusConflict,
    /// The remote kept moving; try again.
    SyncConflict,
    /// The private sync repository is damaged.
    SyncRepoDamaged,
    /// The outcome is not known yet.
    Unresolved,
    /// The command line is wrong.
    Usage,
    /// The input is not valid.
    Validation,
    /// The published journal belongs to another workspace.
    WorkspaceMismatch,
}

impl Code {
    /// Every code, in wire order.
    pub const ALL: [Code; 27] = [
        Code::AlreadyClaimed,
        Code::AttemptFork,
        Code::ClaimedByOther,
        Code::Config,
        Code::Cycle,
        Code::FormatUnsupported,
        Code::GitError,
        Code::Identity,
        Code::Io,
        Code::IssueNotFound,
        Code::JournalFork,
        Code::KeySpace,
        Code::ManifestMissing,
        Code::NotInitialized,
        Code::NotOwner,
        Code::NoRemote,
        Code::NoWorkspace,
        Code::RemoteDamaged,
        Code::RemoteRefused,
        Code::RemoteUnreachable,
        Code::StatusConflict,
        Code::SyncConflict,
        Code::SyncRepoDamaged,
        Code::Unresolved,
        Code::Usage,
        Code::Validation,
        Code::WorkspaceMismatch,
    ];

    /// The code as it is written on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::AlreadyClaimed => "ALREADY_CLAIMED",
            Code::AttemptFork => "ATTEMPT_FORK",
            Code::ClaimedByOther => "CLAIMED_BY_OTHER",
            Code::Config => "CONFIG",
            Code::Cycle => "CYCLE",
            Code::FormatUnsupported => "FORMAT_UNSUPPORTED",
            Code::GitError => "GIT_ERROR",
            Code::Identity => "IDENTITY",
            Code::Io => "IO",
            Code::IssueNotFound => "ISSUE_NOT_FOUND",
            Code::JournalFork => "JOURNAL_FORK",
            Code::KeySpace => "KEY_SPACE",
            Code::ManifestMissing => "MANIFEST_MISSING",
            Code::NoRemote => "NO_REMOTE",
            Code::NoWorkspace => "NO_WORKSPACE",
            Code::NotInitialized => "NOT_INITIALIZED",
            Code::NotOwner => "NOT_OWNER",
            Code::RemoteDamaged => "REMOTE_DAMAGED",
            Code::RemoteRefused => "REMOTE_REFUSED",
            Code::RemoteUnreachable => "REMOTE_UNREACHABLE",
            Code::StatusConflict => "STATUS_CONFLICT",
            Code::SyncConflict => "SYNC_CONFLICT",
            Code::SyncRepoDamaged => "SYNC_REPO_DAMAGED",
            Code::Unresolved => "UNRESOLVED",
            Code::Usage => "USAGE",
            Code::Validation => "VALIDATION",
            Code::WorkspaceMismatch => "WORKSPACE_MISMATCH",
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl crate::state::KeySpace {
    /// `KEY_SPACE`.
    pub fn code(&self) -> Code {
        Code::KeySpace
    }
}

impl crate::key::MintError {
    /// `KEY_SPACE` when no key is free; `VALIDATION` for a number too long to count on from.
    pub fn code(&self) -> Code {
        match self {
            crate::key::MintError::KeySpace => Code::KeySpace,
            crate::key::MintError::NumberTooLong(_) => Code::Validation,
        }
    }
}

impl crate::import::ImportError {
    /// `VALIDATION`: an import fails whole.
    pub fn code(&self) -> Code {
        Code::Validation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_distinct_and_in_wire_order() {
        let names: Vec<&str> = Code::ALL.iter().map(|code| code.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted);
    }
}
