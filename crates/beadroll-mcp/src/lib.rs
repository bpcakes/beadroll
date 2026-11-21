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
mod protocol;
mod tools;

use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::path::PathBuf;

pub use cli::run;
pub use protocol::Server;

/// The bundled HTML document URI declared by the server's tools and resources.
pub const UI_RESOURCE_URI: &str = "ui://beadroll/issues.html";

/// The MIME type mandated by the MCP Apps specification for HTML UI resources.
pub const UI_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";

/// Maximum number of ready issues returned by `bead_ready`.
pub const MAX_READY_LIMIT: usize = 25;

/// The bundled single-file MCP App HTML view served at [`UI_RESOURCE_URI`].
pub const UI_HTML: &str = include_str!("../ui/index.html");

/// Explicit configuration bound once at server startup.
///
/// The library never reads ambient process environment variables or the current working directory;
/// [`run`] constructs `Config` from command-line flags and startup state and passes it down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Absolute path to the checkout directory every `bead` invocation runs in.
    pub repo: PathBuf,
    /// Path or executable name of the `bead` CLI binary.
    pub bead: PathBuf,
    /// Environment passed to `bead` child processes, with git routing variables scrubbed.
    pub env: Vec<(OsString, OsString)>,
}

impl Config {
    /// Builds a `Config` for `repo` and `bead`, scrubbing git routing variables from `env` so
    /// ambient `GIT_DIR` or `GIT_WORK_TREE` settings never redirect `bead` to another repository.
    pub fn new(
        repo: PathBuf,
        bead: PathBuf,
        env: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> Self {
        Self {
            repo,
            bead,
            env: scrub_git_routing(env),
        }
    }
}

/// Git repository-routing environment variables that could silently redirect `bead` away from the
/// checkout bound in [`Config::repo`].
const GIT_ROUTING_VARS: [&str; 7] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_SUPER_PREFIX",
];

/// Drops git repository-routing variables from `env` while preserving `PATH`, `HOME`, `BEAD_*`,
/// and any other environment variables `bead` or git transport helpers rely on.
pub(crate) fn scrub_git_routing(
    env: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    env.into_iter()
        .filter(|(key, _)| {
            key.to_str()
                .is_none_or(|name| !GIT_ROUTING_VARS.contains(&name))
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
