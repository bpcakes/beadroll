//! Black-box tests of bead, grouped under `tests/cli/`. Module and test names preserve
//! findings-ledger references.
//!
//! Tests use `beadroll-test-support` with the binary this package builds, unless BEAD_UNDER_TEST
//! or a machine selects another. Goldens and parity fixtures pin output and stored bytes. Tests
//! requiring this build's failpoints or in-process storage APIs select [`rust_only`].
#![cfg(test)]

mod children;
mod commands;
mod compat;
mod golden;
mod hardening;
mod parity;
mod protocol;
mod replay;
mod storage;
mod stress;
mod support;
mod sweep;

use beadroll_test_support::{Machine, Sandbox};

/// A new sandbox whose machines run the `bead` this package builds, unless `BEAD_UNDER_TEST` or a
/// machine names another. Only this test root can supply that default: Cargo defines
/// `CARGO_BIN_EXE_bead` for the integration tests of the package that builds `bead`, never for the
/// shared support crate.
pub(crate) fn sandbox() -> Sandbox {
    Sandbox::new(env!("CARGO_BIN_EXE_bead"))
}

/// Selects this package's binary for a test that uses failpoints, reads the store in-process or
/// checks a build-specific contract. An arbitrary BEAD_UNDER_TEST need not share those
/// interfaces.
pub(crate) fn rust_only(sandbox: &Sandbox, name: &str) -> Machine {
    Machine::new(sandbox, name).with_bead(env!("CARGO_BIN_EXE_bead"))
}
