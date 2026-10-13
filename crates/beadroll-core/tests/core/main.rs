//! Integration tests of beadroll-core's public APIs, using events from beadroll-test-support.
//! Test names preserve findings-ledger references (`docs/FINDINGS.md`).
#![cfg(test)]

mod commands;
mod compat;
mod hardening;
mod protocol;
mod replay;
