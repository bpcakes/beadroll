//! Command-line parsing and startup environment capture for `beadroll-mcp`.
//!
//! This is the only module in the crate that reads process arguments, the startup working
//! directory, or environment variables.

use std::ffi::OsString;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Arg, Command};

use crate::{Config, run_stdio};

/// Version reported by `--version` and MCP `serverInfo`.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Parses arguments, binds the session checkout once at startup, and serves MCP over stdio.
pub fn run() -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            write_stderr(&format!(
                "beadroll-mcp: cannot determine startup working directory: {error}\n"
            ));
            return ExitCode::FAILURE;
        }
    };
    let config = match parse_args(std::env::args_os(), cwd, std::env::vars_os()) {
        Ok(ParseOutcome::Run(config)) => config,
        Ok(ParseOutcome::Printed(text)) => {
            write_stdout(&text);
            return ExitCode::SUCCESS;
        }
        Err(UsageError { text, exit_code }) => {
            write_stderr(&text);
            return ExitCode::from(exit_code);
        }
    };

    let stdin = io::stdin();
    let stdout = io::stdout();
    match run_stdio(stdin.lock(), stdout.lock(), config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if is_closed_pipe(&error) => ExitCode::SUCCESS,
        Err(error) => {
            write_stderr(&format!("beadroll-mcp: I/O error: {error}\n"));
            ExitCode::FAILURE
        }
    }
}

/// Outcome of parsing the command line.
#[derive(Debug, PartialEq, Eq)]
enum ParseOutcome {
    /// Start the MCP server with this bound configuration.
    Run(Config),
    /// `--help` or `--version` text to write to standard output before exiting 0.
    Printed(String),
}

/// Command-line usage or argument error.
#[derive(Debug, PartialEq, Eq)]
struct UsageError {
    text: String,
    exit_code: u8,
}

/// Builds the clap command specification for `beadroll-mcp`.
fn command() -> Command {
    Command::new("beadroll-mcp")
        .version(VERSION)
        .about("Read-only MCP App server for browsing beadroll issues in a bound checkout")
        .arg(
            Arg::new("repo")
                .long("repo")
                .value_name("DIR")
                .value_parser(clap::value_parser!(PathBuf))
                .help("Checkout directory to bind this server session to (default: startup cwd)"),
        )
        .arg(
            Arg::new("bead")
                .long("bead")
                .value_name("PATH")
                .value_parser(clap::value_parser!(PathBuf))
                .help("Path to the bead executable (default: `bead` on PATH)"),
        )
}

/// Parses `args` using `cwd` to resolve the bound repository path and captures `env`.
fn parse_args(
    args: impl IntoIterator<Item = OsString>,
    cwd: PathBuf,
    env: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<ParseOutcome, UsageError> {
    let matches = match command().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => {
            let rendered = error.render().to_string();
            return match error.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                    Ok(ParseOutcome::Printed(rendered))
                }
                _ => Err(UsageError {
                    text: rendered,
                    exit_code: 2,
                }),
            };
        }
    };

    let repo = match matches.get_one::<PathBuf>("repo") {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => cwd.join(path),
        None => cwd,
    };
    let bead = matches
        .get_one::<PathBuf>("bead")
        .cloned()
        .unwrap_or_else(|| PathBuf::from("bead"));

    Ok(ParseOutcome::Run(Config::new(repo, bead, env)))
}

/// Whether `error` indicates a closed stdout pipe or hung-up terminal, which exits 0 quietly.
pub(crate) fn is_closed_pipe(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
    )
}

/// Writes `text` to standard output, ignoring a closed pipe.
fn write_stdout(text: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// Writes `text` to standard error, ignoring write failures.
fn write_stderr(text: &str) {
    let mut err = io::stderr().lock();
    let _ = err.write_all(text.as_bytes());
    let _ = err.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn defaults_bind_to_startup_cwd_and_bead_on_path() {
        let env = vec![
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            (OsString::from("GIT_DIR"), OsString::from("/other/.git")),
            (OsString::from("GIT_WORK_TREE"), OsString::from("/other")),
            (OsString::from("BEAD_HOME"), OsString::from("/tmp/home")),
        ];
        let outcome = parse_args(os(&["beadroll-mcp"]), PathBuf::from("/work/repo"), env)
            .expect("valid defaults");
        let ParseOutcome::Run(config) = outcome else {
            panic!("expected Run");
        };
        assert_eq!(config.repo, PathBuf::from("/work/repo"));
        assert_eq!(config.bead, PathBuf::from("bead"));
        assert_eq!(
            config.env,
            vec![
                (OsString::from("PATH"), OsString::from("/usr/bin")),
                (OsString::from("BEAD_HOME"), OsString::from("/tmp/home")),
            ]
        );
    }

    #[test]
    fn explicit_relative_repo_resolves_against_startup_cwd() {
        let outcome = parse_args(
            os(&["beadroll-mcp", "--repo", "sub/wt", "--bead", "/opt/bead"]),
            PathBuf::from("/work"),
            Vec::new(),
        )
        .expect("valid flags");
        let ParseOutcome::Run(config) = outcome else {
            panic!("expected Run");
        };
        assert_eq!(config.repo, PathBuf::from("/work/sub/wt"));
        assert_eq!(config.bead, PathBuf::from("/opt/bead"));
    }

    #[test]
    fn unknown_flag_returns_exit_code_2() {
        let err = parse_args(
            os(&["beadroll-mcp", "--bogus"]),
            PathBuf::from("/work"),
            Vec::new(),
        )
        .expect_err("unknown flag");
        assert_eq!(err.exit_code, 2);
        assert!(err.text.contains("--bogus"));
    }
}
