//! In-process sync API tests using Link, Tx, locked, coordinated and synchronise around sandbox
//! machines. Inject faults through Settings, never the process environment. Link tests check
//! persistent git lifetimes; memo tests check that unchanged journal files are not rehashed; count tests
//! check the unpublished count against a count of the journal the slow way, on one machine and on
//! two (count_foreign); reads
//! tests count how often a sync and a write read the store's state; tx tests a transaction's state
//! while it emits.
//!
//! Machines use BEAD_UNDER_TEST when set, else beadroll_test_support::built_bead. Workspace
//! nextest builds bead; running only this package requires `cargo build -p beadroll` first. A
//! missing binary fails sandbox setup with instructions.
#![cfg(test)]

mod count;
mod count_foreign;
mod failpoints;
mod link;
mod memo;
mod protocol;
mod reads;
mod support;
mod tx;
