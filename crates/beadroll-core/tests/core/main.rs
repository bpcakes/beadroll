//! Integration tests of beadroll-core's public APIs, using events from beadroll-test-support.
//! Test names preserve findings-ledger references (`docs/RUST-PORT.md` §4.2).
#![cfg(test)]

mod commands;
mod compat;
mod hardening;
mod protocol;
mod replay;
