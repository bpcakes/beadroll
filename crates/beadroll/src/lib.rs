//! The `bead` command: `commands` maps typed requests to typed outcomes;
//! `cli` parses arguments, renders results and owns the output streams and exit codes; `sys` is
//! what it needs of the operating system around `main`. The library target exists for the binary,
//! its tests, and `cargo xtask bead`, which runs it in a process that counts its heap.

mod cli;
mod commands;
mod sys;
#[cfg(test)]
mod testing;

use std::ffi::OsString;
use std::process::ExitCode;

/// Runs `bead` with the process's arguments and returns its exit status.
pub fn run() -> ExitCode {
    prepare();
    cli::main()
}

/// Runs `bead` with `argv`, its arguments after the program's name, in this process, as [`run`]
/// runs it with the process's: what `cargo xtask bead` counts the heap of (roll-lqjj.9). Unlike
/// [`run`] it keeps nothing for the process's end: whatever the command made is freed when it
/// returns, so that running it again does not pile one run on another (roll-lqjj.32).
pub fn run_with(argv: Vec<OsString>) -> ExitCode {
    prepare();
    cli::main_with(argv)
}

/// What the process needs before a command runs.
fn prepare() {
    sys::ignore_file_size_signal();
    sys::keep_inherited_descriptors_from_children();
}
