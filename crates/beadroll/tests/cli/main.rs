//! Black-box tests of the `bead` binary: one test binary for the crate, its modules under
//! `tests/cli/` (`docs/RUST-PORT.md` §4.3), most of them ported from the Python suite under the
//! naming rule of §4.2 (`<module>::<class>::<method>`, the Python test's names in snake case).
//!
//! Every test runs `bead` through `beadroll-test-support`: the binary this package builds, unless
//! `BEAD_UNDER_TEST` or a machine names another. Until the cutover a ported test counted only once
//! it also passed against Python's launcher; the goldens (`golden.rs`) and the pinned bytes of
//! `parity` keep what Python answered. A `rust_only` test injects its faults through failpoints,
//! which are this build's, and runs the binary this package builds whatever `BEAD_UNDER_TEST` says
//! ([`rust_only`]).
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

/// A machine of a `rust_only` test (`docs/RUST-PORT.md` §4.2): one that injects a fault through a
/// failpoint (`BEAD_FAILPOINTS`, §4.5), reads its store in this process, or checks one of the Rust
/// bead's own departures from Python (§5 row 26), so it runs the `bead` this package builds
/// whatever `BEAD_UNDER_TEST` says: those are this build's, which another binary under test (an
/// installed bead, or `python-0.1.0`'s launcher) need not share. A test that replaces a Python
/// test names it, as the one it was reviewed against.
pub(crate) fn rust_only(sandbox: &Sandbox, name: &str) -> Machine {
    Machine::new(sandbox, name).with_bead(env!("CARGO_BIN_EXE_bead"))
}
