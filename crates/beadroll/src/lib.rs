//! The `bead` command (`docs/RUST-PORT.md` §2.1): `commands` maps typed requests to typed outcomes;
//! `cli` parses arguments, renders results and owns the output streams and exit codes. The library
//! target exists for the binary and its tests.
//!
//! Phase 2a ports `init`, `create`, `show`, `list`, `ready` and `sync`; the other commands follow in
//! Phases 2b and 2c, and until then the parser does not know them.

mod cli;
mod commands;

use std::process::ExitCode;

/// Runs `bead` with the process's arguments and returns its exit status.
pub fn run() -> ExitCode {
    cli::main()
}
