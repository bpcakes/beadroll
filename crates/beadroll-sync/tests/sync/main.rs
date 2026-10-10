//! The Python suite's in-crate API tests of the sync protocol: the eleven C-api tests of
//! `test_protocol` (`docs/RUST-PORT.md` §4.3), named by the inventory's rule
//! (`xtask/inventory.txt`). Each drives this crate's API in this process (`Link`, `Tx`, `locked`,
//! `coordinated`, `synchronise`) on one machine's store, while the machines around it run `bead`.
//! Where Python patched a function to inject a fault, a failpoint does it, and the registry reaches
//! the API through `Settings`, never through the process environment (§4.5).
//!
//! The machines run `BEAD_UNDER_TEST` when it is set, else the checkout's Python launcher
//! (`./bead`): Cargo names the `bead` it builds only to the `beadroll` package's own tests, and
//! Python's is the implementation whose stores this crate must share.
#![cfg(test)]

mod failpoints;
mod protocol;
mod support;
