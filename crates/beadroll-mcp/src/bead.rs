//! Subprocess boundary to the installed `bead` CLI for strictly local read operations.

use std::ffi::OsString;
use std::io;
use std::time::Duration;

use serde_json::{Value, json};
use thiserror::Error;

use crate::model::{DetailView, IssueDetail, ReadyIssue, ReadyView, WorkspaceInfo};
use crate::process::{ProcessError, run_bounded};
use crate::{Config, MAX_OUTPUT_BYTES};

/// Target for `bead_show`, keeping explicit stable `uid` lookups distinct from initial display-key
/// lookups so a display key colliding with a UID can never open the wrong issue silently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ShowTarget {
    /// Stable issue UID from a previous `bead_ready` or `bead_show` response.
    Uid(String),
    /// User- or model-supplied issue key (or UID) for an initial lookup.
    Id(String),
}

impl ShowTarget {
    fn as_arg(&self) -> &str {
        match self {
            Self::Uid(uid) => uid.as_str(),
            Self::Id(id) => id.as_str(),
        }
    }
}

/// Actionable failure produced when reading the bound checkout's beadroll tracker.
#[derive(Debug, Error)]
pub(crate) enum AppError {
    /// Tool arguments failed validation (returned as an MCP tool error so models can recover).
    #[error("{message}")]
    InvalidArgument {
        /// Validation failure description.
        message: String,
    },
    /// The checkout path configured at server startup is missing or not a directory.
    #[error("Bound checkout directory is not accessible: {path}")]
    CheckoutUnavailable {
        /// Configured checkout path.
        path: String,
    },
    /// The configured `bead` binary could not be found on `PATH` or at `--bead`.
    #[error(
        "bead executable not found at `{program}`. Install beadroll (see README Install) or configure `--bead PATH`."
    )]
    BeadNotFound {
        /// Program name or path that failed to spawn.
        program: String,
    },
    /// Spawning `bead` failed for an operating-system reason other than missing executable.
    #[error("Failed to start `{program}`: {detail}")]
    SpawnFailed {
        /// Program name or path.
        program: String,
        /// Underlying OS error text.
        detail: String,
    },
    /// A `bead` subprocess exceeded the server's wall-clock deadline.
    #[error("`{command}` did not finish within {} ms and was terminated.", timeout.as_millis())]
    TimedOut {
        /// Subcommand that timed out.
        command: &'static str,
        /// Configured timeout duration.
        timeout: Duration,
        /// Workspace metadata, when `bead info` had already succeeded.
        workspace: Option<Box<WorkspaceInfo>>,
    },
    /// A `bead` subprocess exceeded the maximum captured output size.
    #[error("`{command}` output exceeded the {limit}-byte capture limit.")]
    OutputTooLarge {
        /// Subcommand whose output exceeded the limit.
        command: &'static str,
        /// Byte limit per stream.
        limit: usize,
        /// Workspace metadata, when `bead info` had already succeeded.
        workspace: Option<Box<WorkspaceInfo>>,
    },
    /// The `bead` CLI returned a structured or plain error.
    #[error("{message}")]
    BeadCommand {
        /// Structured bead error code (e.g. `NOT_INITIALIZED`, `ISSUE_NOT_FOUND`, `NO_WORKSPACE`).
        code: String,
        /// Actionable message from `bead`.
        message: String,
        /// Workspace metadata, when `bead info` succeeded before the failing command.
        workspace: Option<Box<WorkspaceInfo>>,
    },
    /// The installed `bead` does not emit stable `uid` fields required for safe issue reopening.
    #[error(
        "Installed bead ({version}) does not output stable issue UIDs required by beadroll-mcp. Upgrade bead to a build with stable issue UIDs and restart the MCP server."
    )]
    UpgradeRequired {
        /// Version string reported by `bead info`, or `"unknown"`.
        version: String,
        /// Workspace metadata from `bead info`.
        workspace: Option<Box<WorkspaceInfo>>,
    },
    /// A `bead_show` call carried a `workspace` ID belonging to another tracker.
    #[error(
        "Requested workspace `{requested}` does not match this server's bound workspace `{actual}` ({checkout}). Start an MCP session bound to that checkout instead."
    )]
    WorkspaceMismatch {
        /// Workspace ID supplied by the caller.
        requested: String,
        /// Workspace ID of the checkout bound to this server session.
        actual: String,
        /// Checkout directory bound to this server session.
        checkout: String,
        /// Workspace metadata of the bound checkout.
        workspace: Box<WorkspaceInfo>,
    },
    /// An explicit `uid` lookup matched an issue with a different `uid` (for example, an imported
    /// display key colliding with the requested UID string).
    #[error(
        "Stable UID lookup `{requested}` resolved to issue `{matched_id}` with different UID `{actual}` (a display key collided with that UID)."
    )]
    UidMismatch {
        /// UID explicitly requested by the caller.
        requested: String,
        /// Display key of the issue `bead show` returned.
        matched_id: String,
        /// Actual UID of the issue `bead show` returned.
        actual: String,
        /// Workspace metadata of the bound checkout.
        workspace: Box<WorkspaceInfo>,
    },
    /// `bead` exited 0 but produced unexpected JSON.
    #[error("Unexpected JSON from `{command}`: {detail}")]
    InvalidOutput {
        /// Subcommand whose stdout could not be parsed.
        command: &'static str,
        /// Short diagnostic describing the malformed output.
        detail: String,
    },
}

impl AppError {
    /// Machine-readable error code for `structuredContent` and tests.
    pub(crate) fn code(&self) -> &str {
        match self {
            Self::InvalidArgument { .. } => "INVALID_ARGUMENT",
            Self::CheckoutUnavailable { .. } => "CHECKOUT_UNAVAILABLE",
            Self::BeadNotFound { .. } => "BEAD_NOT_FOUND",
            Self::SpawnFailed { .. } => "SPAWN_FAILED",
            Self::TimedOut { .. } => "BEAD_TIMEOUT",
            Self::OutputTooLarge { .. } => "BEAD_OUTPUT_TOO_LARGE",
            Self::BeadCommand { code, .. } => code.as_str(),
            Self::UpgradeRequired { .. } => "UPGRADE_REQUIRED",
            Self::WorkspaceMismatch { .. } => "WORKSPACE_MISMATCH",
            Self::UidMismatch { .. } => "UID_MISMATCH",
            Self::InvalidOutput { .. } => "INVALID_BEAD_OUTPUT",
        }
    }

    pub(crate) fn invalid_argument(message: impl Into<String>) -> Self {
        Self::InvalidArgument {
            message: message.into(),
        }
    }

    pub(crate) fn upgrade_required(version: Option<&str>) -> Self {
        Self::UpgradeRequired {
            version: version.unwrap_or("unknown").to_owned(),
            workspace: None,
        }
    }

    pub(crate) fn invalid_output(command: &'static str, detail: impl Into<String>) -> Self {
        Self::InvalidOutput {
            command,
            detail: detail.into(),
        }
    }

    /// Attaches `workspace` metadata to an error when `bead info` had already succeeded.
    pub(crate) fn with_workspace(self, ws: WorkspaceInfo) -> Self {
        match self {
            Self::BeadCommand { code, message, .. } => Self::BeadCommand {
                code,
                message,
                workspace: Some(Box::new(ws)),
            },
            Self::UpgradeRequired { version, .. } => Self::UpgradeRequired {
                version,
                workspace: Some(Box::new(ws)),
            },
            Self::TimedOut {
                command, timeout, ..
            } => Self::TimedOut {
                command,
                timeout,
                workspace: Some(Box::new(ws)),
            },
            Self::OutputTooLarge { command, limit, .. } => Self::OutputTooLarge {
                command,
                limit,
                workspace: Some(Box::new(ws)),
            },
            other => other,
        }
    }

    /// Serializes this error as an MCP App `structuredContent` error payload so the UI renders a
    /// themed, actionable error card instead of raw text.
    pub(crate) fn to_structured_json(&self) -> Value {
        let workspace = match self {
            Self::BeadCommand { workspace, .. }
            | Self::UpgradeRequired { workspace, .. }
            | Self::TimedOut { workspace, .. }
            | Self::OutputTooLarge { workspace, .. } => {
                workspace.as_deref().map(WorkspaceInfo::to_json)
            }
            Self::WorkspaceMismatch { workspace, .. } | Self::UidMismatch { workspace, .. } => {
                Some(workspace.to_json())
            }
            Self::InvalidArgument { .. }
            | Self::CheckoutUnavailable { .. }
            | Self::BeadNotFound { .. }
            | Self::SpawnFailed { .. }
            | Self::InvalidOutput { .. } => None,
        };
        json!({
            "view": "error",
            "error": {
                "code": self.code(),
                "message": self.to_string(),
            },
            "workspace": workspace,
        })
    }
}

/// Runs strictly local read commands against the checkout bound in [`Config`].
pub(crate) struct BeadCli {
    config: Config,
}

impl BeadCli {
    /// Creates a runner bound to `config`.
    pub(crate) fn new(config: Config) -> Self {
        Self { config }
    }

    /// Reads workspace metadata and up to `limit` ready issues in beadroll's order, pinning the
    /// `bead ready` subprocess to the exact workspace ID resolved by `bead info`.
    pub(crate) fn ready(&self, limit: usize) -> Result<ReadyView, AppError> {
        let workspace = self.info()?;
        let fetch_limit = (limit + 1).to_string();
        let raw = self
            .run_json_pinned(
                "bead ready",
                &["ready", "--limit", &fetch_limit, "--json"],
                Some(&workspace.id),
            )
            .map_err(|error| error.with_workspace(workspace.clone()))?;
        let items = raw
            .as_array()
            .ok_or_else(|| AppError::invalid_output("bead ready", "expected a JSON array"))?;
        let has_more = items.len() > limit;
        let version = workspace.bead_version.as_deref();
        let mut issues = Vec::with_capacity(items.len().min(limit));
        for item in items.iter().take(limit) {
            let issue = ReadyIssue::from_json(item, version)
                .map_err(|error| error.with_workspace(workspace.clone()))?;
            issues.push(issue);
        }
        Ok(ReadyView {
            workspace,
            limit,
            has_more,
            issues,
        })
    }

    /// Reads workspace metadata, verifies `expected_workspace` when provided, pins `bead show` to
    /// that exact workspace ID, and verifies the returned `uid` when `target` is an explicit UID.
    pub(crate) fn show(
        &self,
        target: &ShowTarget,
        expected_workspace: Option<&str>,
    ) -> Result<DetailView, AppError> {
        let workspace = self.info()?;
        if let Some(expected) = expected_workspace
            && expected != workspace.id
        {
            return Err(AppError::WorkspaceMismatch {
                requested: expected.to_owned(),
                actual: workspace.id.clone(),
                checkout: workspace.checkout.clone(),
                workspace: Box::new(workspace),
            });
        }
        let raw = self
            .run_json_pinned(
                "bead show",
                &["show", "--json", "--", target.as_arg()],
                Some(&workspace.id),
            )
            .map_err(|error| error.with_workspace(workspace.clone()))?;
        let items = raw
            .as_array()
            .ok_or_else(|| AppError::invalid_output("bead show", "expected a JSON array"))?;
        let first = items
            .first()
            .ok_or_else(|| AppError::invalid_output("bead show", "empty issue array"))?;
        let issue = IssueDetail::from_json(first, workspace.bead_version.as_deref())
            .map_err(|error| error.with_workspace(workspace.clone()))?;
        if let ShowTarget::Uid(requested) = target
            && issue.uid != *requested
        {
            return Err(AppError::UidMismatch {
                requested: requested.clone(),
                matched_id: issue.id,
                actual: issue.uid,
                workspace: Box::new(workspace),
            });
        }
        Ok(DetailView { workspace, issue })
    }

    /// Reads workspace identity and last-sync metadata from `bead info --json` without any
    /// `BEAD_WORKSPACE` override so the checkout itself determines the workspace ID.
    fn info(&self) -> Result<WorkspaceInfo, AppError> {
        let raw = self.run_json_pinned("bead info", &["info", "--json"], None)?;
        let checkout = self.config.repo.display().to_string();
        WorkspaceInfo::from_info_json(&raw, checkout)
    }

    /// Spawns `bead` in the bound checkout under the configured timeout and output bounds,
    /// optionally pinning `BEAD_WORKSPACE` to `pinned_workspace` so a concurrent checkout config
    /// change between `info` and `ready`/`show` cannot read a different store.
    fn run_json_pinned(
        &self,
        label: &'static str,
        args: &[&str],
        pinned_workspace: Option<&str>,
    ) -> Result<Value, AppError> {
        // Distinguish a missing checkout directory from a missing `bead` binary before spawn:
        // both surface as `io::ErrorKind::NotFound` from `Command::spawn`.
        if !self.config.repo.is_dir() {
            return Err(AppError::CheckoutUnavailable {
                path: self.config.repo.display().to_string(),
            });
        }
        let mut env = self.config.env.clone();
        if let Some(ws_id) = pinned_workspace {
            env.push((OsString::from("BEAD_WORKSPACE"), OsString::from(ws_id)));
        }
        let output = run_bounded(
            &self.config.bead,
            args,
            &self.config.repo,
            &env,
            self.config.timeout,
            MAX_OUTPUT_BYTES,
        )
        .map_err(|error| match error {
            ProcessError::Spawn(io_err) => {
                map_spawn_error(&self.config.bead.display().to_string(), &io_err)
            }
            ProcessError::TimedOut { timeout } => AppError::TimedOut {
                command: label,
                timeout,
                workspace: None,
            },
            ProcessError::OutputTooLarge { limit } => AppError::OutputTooLarge {
                command: label,
                limit,
                workspace: None,
            },
        })?;

        if !output.status.success() {
            return Err(parse_bead_failure(&output.stderr, &output.stdout));
        }
        let stdout = String::from_utf8(output.stdout).map_err(|error| {
            AppError::invalid_output(label, format!("stdout is not UTF-8: {error}"))
        })?;
        serde_json::from_str(&stdout)
            .map_err(|error| AppError::invalid_output(label, format!("invalid JSON: {error}")))
    }
}

fn map_spawn_error(program: &str, error: &io::Error) -> AppError {
    if error.kind() == io::ErrorKind::NotFound {
        AppError::BeadNotFound {
            program: program.to_owned(),
        }
    } else {
        AppError::SpawnFailed {
            program: program.to_owned(),
            detail: error.to_string(),
        }
    }
}

/// Extracts `{"error": {"code", "message"}}` printed on stderr by `bead --json`, falling back to
/// raw stderr/stdout text when a non-JSON error occurred.
fn parse_bead_failure(stderr: &[u8], stdout: &[u8]) -> AppError {
    let stderr_text = String::from_utf8_lossy(stderr).trim().to_owned();
    for line in stderr_text.lines().rev() {
        let trimmed = line.trim();
        if let Ok(Value::Object(root)) = serde_json::from_str::<Value>(trimmed)
            && let Some(Value::Object(err)) = root.get("error")
            && let Some(code) = err.get("code").and_then(Value::as_str)
            && let Some(message) = err.get("message").and_then(Value::as_str)
        {
            return AppError::BeadCommand {
                code: code.to_owned(),
                message: message.to_owned(),
                workspace: None,
            };
        }
    }
    let fallback = if !stderr_text.is_empty() {
        stderr_text
    } else {
        let out_text = String::from_utf8_lossy(stdout).trim().to_owned();
        if !out_text.is_empty() {
            out_text
        } else {
            "bead exited with an error and produced no message".to_owned()
        }
    };
    AppError::BeadCommand {
        code: "BEAD_ERROR".to_owned(),
        message: fallback,
        workspace: None,
    }
}
