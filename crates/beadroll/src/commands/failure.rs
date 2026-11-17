//! What a command that failed reports: its code, its message (with the bytes of one that is not
//! UTF-8), and the conversions from every crate's errors.

use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::code::Code;
use beadroll_core::text::printed_text;
use beadroll_store::ViewError;

/// A command failure: the code and message bead reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) code: Code,
    pub(crate) message: String,
    /// The original message bytes when a command-line argument or path is not UTF-8. `message`
    /// substitutes U+FFFD; human output uses `?` for each such byte, and JSON `\ufffd`.
    pub(crate) raw: Option<Vec<u8>>,
    /// What a human reads after the message of a usage error: the usage, and where help is.
    pub(crate) after: Option<String>,
}

impl Failure {
    pub(crate) fn new(code: Code, message: impl Into<String>) -> Failure {
        Failure {
            code,
            message: message.into(),
            raw: None,
            after: None,
        }
    }

    /// A usage error: `message`, bytes that may not all be UTF-8, then `after` for a human (the
    /// usage, and where help is).
    pub(crate) fn usage(message: impl Into<Vec<u8>>, after: impl Into<String>) -> Failure {
        let after: String = after.into();
        Failure {
            after: Some(after).filter(|after| !after.is_empty()),
            ..Failure::bytes(Code::Usage, message.into())
        }
    }

    /// A failure whose message is `raw`, bytes that may not all be UTF-8.
    pub(crate) fn bytes(code: Code, raw: Vec<u8>) -> Failure {
        match String::from_utf8(raw) {
            Ok(message) => Failure::new(code, message),
            Err(error) => Failure {
                code,
                message: String::from_utf8_lossy(error.as_bytes()).into_owned(),
                raw: Some(error.into_bytes()),
                after: None,
            },
        }
    }

    /// The message for human output, replacing each byte that is not UTF-8 with `?`.
    pub(crate) fn printed(&self) -> String {
        match &self.raw {
            Some(raw) => printed_text(raw),
            None => self.message.clone(),
        }
    }
}

/// An operating-system error formatted as `{strerror}: {path}`, retaining the path's original
/// bytes.
fn os_failure(error: &beadroll_git::IoError) -> Failure {
    match &error.path {
        Some(path) => {
            let mut raw = format!("{}: ", error.strerror).into_bytes();
            raw.extend_from_slice(path.as_os_str().as_bytes());
            Failure::bytes(Code::Io, raw)
        }
        None => Failure::new(Code::Io, error.to_string()),
    }
}

impl From<beadroll_sync::Error> for Failure {
    fn from(error: beadroll_sync::Error) -> Failure {
        match error {
            beadroll_sync::Error::Store(error) => error.into(),
            error => Failure::new(error.code(), error.to_string()),
        }
    }
}

impl From<beadroll_store::Error> for Failure {
    fn from(error: beadroll_store::Error) -> Failure {
        match &error {
            beadroll_store::Error::Io(io) => os_failure(io),
            _ => Failure::new(error.code(), error.to_string()),
        }
    }
}

/// A value that cannot be written as JSON is the data's fault (`VALIDATION`), as the store says.
impl From<beadroll_core::json::WriteError> for Failure {
    fn from(error: beadroll_core::json::WriteError) -> Failure {
        beadroll_store::Error::from(error).into()
    }
}

impl From<beadroll_git::Error> for Failure {
    fn from(error: beadroll_git::Error) -> Failure {
        Failure::new(beadroll_store::git_code(&error), error.to_string())
    }
}

impl From<beadroll_git::IoError> for Failure {
    fn from(error: beadroll_git::IoError) -> Failure {
        os_failure(&error)
    }
}

impl From<ViewError> for Failure {
    fn from(error: ViewError) -> Failure {
        match error {
            ViewError::Store(error) => error.into(),
            ViewError::KeySpace(error) => Failure::new(error.code(), error.to_string()),
        }
    }
}

impl From<beadroll_core::key::MintError> for Failure {
    fn from(error: beadroll_core::key::MintError) -> Failure {
        Failure::new(error.code(), error.to_string())
    }
}

impl From<beadroll_core::state::MissingTexts> for Failure {
    fn from(error: beadroll_core::state::MissingTexts) -> Self {
        Self::new(error.code(), error.to_string())
    }
}

impl From<beadroll_core::import::ImportError> for Failure {
    fn from(error: beadroll_core::import::ImportError) -> Failure {
        Failure::new(error.code(), error.to_string())
    }
}
