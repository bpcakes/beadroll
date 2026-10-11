//! The Python suite's in-crate API tests of the sync protocol: the eleven C-api tests of
//! `test_protocol` (`docs/RUST-PORT.md` §4.3), named by the naming rule of §4.2. Each drives this
//! crate's API in this process (`Link`, `Tx`, `locked`, `coordinated`, `synchronise`) on one
//! machine's store, while the machines around it run `bead`.
//! Where Python patched a function to inject a fault, a failpoint does it, and the registry reaches
//! the API through `Settings`, never through the process environment (§4.5). Beside them, `link`
//! checks what Python has no counterpart of: the long-lived gits a link keeps (Appendix B) end with
//! it.
//!
//! The machines run `BEAD_UNDER_TEST` when it is set, else the `bead` Cargo built in the profile
//! directory above this test binary (`beadroll_test_support::built_bead`): Cargo names the `bead`
//! it builds only to the `beadroll` package's own tests. `cargo nextest run --workspace` builds it;
//! run on this package alone, the tests need it built first (`cargo build -p beadroll`), and a
//! sandbox fails at once, saying so, when it is not there.
#![cfg(test)]

mod failpoints;
mod link;
mod protocol;
mod support;
