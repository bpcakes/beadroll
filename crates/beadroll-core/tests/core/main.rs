//! beadroll-core's integration tests: the Python suite's tests of replay and the core, ported under
//! the inventory's naming rule (`docs/RUST-PORT.md` §4.3, `xtask/inventory.txt`), each against the
//! public API with events built by `beadroll-test-support`.
#![cfg(test)]

mod commands;
mod compat;
mod hardening;
mod protocol;
mod replay;
