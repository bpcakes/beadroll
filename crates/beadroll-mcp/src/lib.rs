//! Read-only MCP App server for browsing a checkout's beadroll tracker.
//!
//! `beadroll-mcp` speaks newline-delimited JSON-RPC 2.0 over standard input and output, binds one
//! checkout at startup ([`Config`]), shells out to the installed `bead` binary for strictly local
//! reads (`bead info`, `bead ready`, `bead show`), and serves a self-contained HTML view over the
//! MCP Apps `ui://` resource scheme.

mod bead;
mod cli;
mod format;
mod model;
mod process;
mod protocol;
mod tools;

use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

pub use cli::run;
pub use protocol::Server;

/// The bundled HTML document URI declared by the server's tools and resources.
pub const UI_RESOURCE_URI: &str = "ui://beadroll/issues.html";

/// The MIME type mandated by the MCP Apps specification for HTML UI resources.
pub const UI_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";

/// Maximum number of ready issues returned by `bead_ready`.
pub const MAX_READY_LIMIT: usize = 25;

/// Maximum incoming JSON-RPC stdio frame payload size in bytes (1 MiB, excluding line endings).
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Default wall-clock timeout for each local `bead` subprocess invocation.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum stdout or stderr bytes captured from a single `bead` subprocess (16 MiB).
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// The bundled single-file MCP App HTML view served at [`UI_RESOURCE_URI`].
pub const UI_HTML: &str = include_str!("../ui/index.html");

/// Explicit configuration bound once at server startup.
///
/// The library never reads ambient process environment variables or the current working directory;
/// [`run`] constructs `Config` from command-line flags and startup state and passes it down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Absolute (and canonicalized when accessible) path to the checkout directory every `bead`
    /// invocation runs in.
    pub repo: PathBuf,
    /// Path or executable name of the `bead` CLI binary.
    pub bead: PathBuf,
    /// Environment passed to `bead` child processes, with git and beadroll routing/config
    /// overrides scrubbed.
    pub env: Vec<(OsString, OsString)>,
    /// Wall-clock deadline for each `bead` subprocess invocation.
    pub timeout: Duration,
}

impl Config {
    /// Builds a `Config` for `repo` and `bead`, scrubbing git routing/config overrides and all
    /// `BEAD_*` variables except `BEAD_HOME` from `env` so inherited settings cannot redirect or
    /// alter reads of the bound checkout.
    pub fn new(
        repo: PathBuf,
        bead: PathBuf,
        env: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> Self {
        Self {
            repo,
            bead,
            env: scrub_routing_env(env),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Returns this configuration with subprocess timeout set to `timeout`.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Exact environment variable names stripped from `bead` children so ambient git discovery,
/// external git config files, or transport settings cannot redirect or network-fetch during local
/// reads.
const SCRUBBED_EXACT: [&str; 27] = [
    // `git rev-parse --local-env-vars` (same 14 as `beadroll_git::REPO_LOCATION_ENV`)
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_QUARANTINE_PATH",
    // Git discovery, external config files, and transport/lazy-fetch settings
    "GIT_SUPER_PREFIX",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_CONFIG",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_NO_LAZY_FETCH",
    "GIT_ALLOW_PROTOCOL",
    "GIT_TERMINAL_PROMPT",
    // Can supply `$XDG_CONFIG_HOME/git/config`
    "XDG_CONFIG_HOME",
];

/// Prefixes of indexed git config overrides (`GIT_CONFIG_KEY_N`, `GIT_CONFIG_VALUE_N`).
const SCRUBBED_PREFIXES: [&str; 2] = ["GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"];

/// Drops git routing/config variables and all `BEAD_*` variables except `BEAD_HOME` from `env`,
/// preserving `PATH`, `HOME`, `BEAD_HOME`, `TMPDIR`, `LLVM_PROFILE_FILE`, and other runtime vars.
pub(crate) fn scrub_routing_env(
    env: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    env.into_iter()
        .filter(|(key, _)| {
            key.to_str().is_none_or(|name| {
                let is_non_home_bead = name.starts_with("BEAD_") && name != "BEAD_HOME";
                !is_non_home_bead
                    && !SCRUBBED_EXACT.contains(&name)
                    && !SCRUBBED_PREFIXES
                        .iter()
                        .any(|prefix| name.starts_with(prefix))
            })
        })
        .collect()
}

/// Runs the MCP JSON-RPC 2.0 server over `reader` and `writer` until EOF or a broken pipe.
pub fn run_stdio(
    reader: impl BufRead,
    writer: impl Write,
    config: Config,
) -> Result<(), std::io::Error> {
    let mut server = Server::new(config);
    server.serve(reader, writer)
}
