//! The `bead` command (`docs/RUST-PORT.md` §2.1): `commands` maps typed requests to typed outcomes;
//! `cli` parses arguments, renders results and owns the output streams and exit codes; `sys` is
//! what it needs of the operating system around `main`. The library target exists for the binary
//! and its tests.

mod cli;
mod commands;
mod sys;

use std::process::ExitCode;

/// Runs `bead` with the process's arguments and returns its exit status.
pub fn run() -> ExitCode {
    sys::ignore_file_size_signal();
    sys::keep_inherited_descriptors_from_children();
    cli::main()
}
