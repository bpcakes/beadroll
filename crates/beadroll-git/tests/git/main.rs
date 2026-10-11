//! The Python suite's tests of what beadroll-git owns (`docs/RUST-PORT.md` §6, Phase 1's list of
//! B-class tests that land with Phase 2a): the remote URL rules, the runner's timeouts, and the
//! local-damage classifier. Named by the naming rule of `docs/RUST-PORT.md` §4.2.
#![cfg(test)]

mod commands;
mod hardening;
mod replay;
