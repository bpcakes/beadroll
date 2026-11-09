//! Integration tests of beadroll-core's public APIs, using events from beadroll-test-support.
//! Test names preserve findings-ledger references (`docs/FINDINGS.md`).
#![cfg(test)]

mod commands;
mod compat;
mod hardening;
mod hydration;
mod lifecycle;
mod protocol;
mod replay;
mod snapshots;
