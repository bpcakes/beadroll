//! Black-box tests of bead, grouped under `tests/cli/`. A module is named for its subject and a
//! test for the contract it checks; a finding's id is cited in its doc comment.
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

/// The network timeout, in seconds, of a command meant to give up on a push that a test's
/// pre-receive hook holds until it lets the push go: short of that hold, and long enough for every
/// other fetch and push the command makes on a machine loaded by the rest of the suite, which a
/// second was not (roll-t7ws), as `finished_push_is_not_an_unanswered_one` gives ten seconds short
/// of its helper's thirty.
pub(crate) const GIVE_UP_ON_A_HELD_PUSH: &str = "10";
