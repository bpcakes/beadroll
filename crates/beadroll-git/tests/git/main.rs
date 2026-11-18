//! Public API tests for remote URL rules, runner timeouts and local-damage classification. A
//! module is named for its subject and a test for the contract it checks; a finding's id is cited
//! in its doc comment.
#![cfg(test)]

mod commands;
mod hardening;
mod replay;
