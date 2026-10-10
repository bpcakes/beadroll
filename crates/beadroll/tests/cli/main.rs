//! Black-box tests of the `bead` binary: one test binary for the crate, its modules under
//! `tests/cli/` (`docs/RUST-PORT.md` §4.3), each test ported from the Python suite under the
//! inventory's naming rule (`xtask/inventory.txt`).
//!
//! Every test runs `bead` through `beadroll-test-support`: the binary this package builds, unless
//! `BEAD_UNDER_TEST` or a machine names another. A ported test counts only once it also passes
//! against Python's launcher (`BEAD_UNDER_TEST=$PWD/bead cargo nextest run`), and it passes
//! against the Rust binary too. `BEAD_MIXED_WITH` names a second binary for every second machine
//! of a sandbox, so that the protocol scenarios also run on mixed fleets. A `rust_only` test
//! injects its faults through failpoints, which only the Rust binary has, and runs that binary
//! whatever `BEAD_UNDER_TEST` and `BEAD_MIXED_WITH` say ([`rust_only`]).
#![cfg(test)]

mod children;
mod commands;
mod compat;
mod golden;
mod hardening;
mod mixed;
mod protocol;
mod replay;
mod storage;
mod stress;
mod support;
mod sweep;

use beadroll_test_support::{Machine, Sandbox};

/// A new sandbox whose machines run the `bead` this package builds, unless `BEAD_UNDER_TEST`,
/// `BEAD_MIXED_WITH` (for every second machine) or a machine names another. Only this test root
/// can supply that default: Cargo defines `CARGO_BIN_EXE_bead` for the integration tests of the
/// package that builds `bead`, never for the shared support crate.
pub(crate) fn sandbox() -> Sandbox {
    Sandbox::new(env!("CARGO_BIN_EXE_bead"))
}

/// A machine of a `rust_only` test (`docs/RUST-PORT.md` §4.2): one that injects a fault through a
/// failpoint (`BEAD_FAILPOINTS`, §4.5), which only the Rust binary has, so it runs the `bead` this
/// package builds whatever `BEAD_UNDER_TEST` and `BEAD_MIXED_WITH` say. Such a test names the
/// Python test it replaces, against whose assertions it is reviewed.
pub(crate) fn rust_only(sandbox: &Sandbox, name: &str) -> Machine {
    Machine::new(sandbox, name).with_bead(env!("CARGO_BIN_EXE_bead"))
}
