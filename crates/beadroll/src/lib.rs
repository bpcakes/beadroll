//! The `bead` command. `commands` will map typed requests to typed outcomes; `cli` will parse
//! arguments, render results and own the output streams and exit codes; `sys` will hold the two
//! audited unsafe spots (`docs/RUST-PORT.md` §2.1). The library target exists for the binary and
//! its tests.
//!
//! Phase 0 implements no command: [`run`] refuses every invocation, so no test can pass against
//! this binary by accident. Until Phase 2 fills it, the black-box tests run against Python's
//! `./bead` through `BEAD_UNDER_TEST`.

use std::io::Write as _;
use std::process::ExitCode;

/// Runs `bead` with the process's arguments. In Phase 0 that is a refusal: one line on standard
/// error and exit status 1, whatever the arguments.
pub fn run() -> ExitCode {
    // Nothing to do if standard error is closed: the exit status still says what happened.
    let _ = writeln!(
        std::io::stderr(),
        "bead: the Rust implementation has no commands yet (docs/RUST-PORT.md §6)"
    );
    ExitCode::FAILURE
}
