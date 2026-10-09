//! Black-box tests of the `bead` binary: one test binary for the crate, its modules under
//! `tests/cli/` (`docs/RUST-PORT.md` §4.3), each test ported from the Python suite under the
//! inventory's naming rule (`xtask/inventory.txt`).
//!
//! Every test runs `bead` through `beadroll-test-support`: the binary this package builds, unless
//! `BEAD_UNDER_TEST` or a machine names another. A ported test counts only once it also passes
//! against Python's launcher, and until Phase 2 gives the Rust binary its commands that is the
//! only binary they pass against: `BEAD_UNDER_TEST=$PWD/bead cargo nextest run`.
#![cfg(test)]

mod commands;
mod mixed;
mod support;

use beadroll_test_support::Sandbox;

/// A new sandbox whose machines run the `bead` this package builds, unless `BEAD_UNDER_TEST` or a
/// machine names another. Only this test root can supply that default: Cargo defines
/// `CARGO_BIN_EXE_bead` for the integration tests of the package that builds `bead`, never for the
/// shared support crate.
pub(crate) fn sandbox() -> Sandbox {
    Sandbox::new(env!("CARGO_BIN_EXE_bead"))
}
