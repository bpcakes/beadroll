//! Black-box test fixtures: a [`Sandbox`] with a local bare remote and [`Machine`]s with
//! independent homes, clones and identities. Tracker writes stay within the sandbox.
//!
//! Binary selection is explicit: machine override ([`Machine::with_bead`]), then BEAD_UNDER_TEST,
//! then the test binary's default supplied to [`Sandbox::new`]. CLI tests pass
//! CARGO_BIN_EXE_bead; other crates use [`built_bead`]. A missing executable fails setup
//! immediately.
//!
//! [`Json`] reads bead's JSON output; [`json!`] and [`events`] build records for core integration
//! tests, and [`values`] draws the JSON values the round-trip properties write and read back.
#![allow(clippy::unwrap_used, reason = "a failed fixture is a failed test")]
#![allow(clippy::expect_used, reason = "a failed fixture is a failed test")]
#![allow(clippy::panic, reason = "a failed fixture is a failed test")]
#![allow(clippy::print_stderr, reason = "a failed fixture is a failed test")]

pub mod events;
mod gate;
mod isolation;
mod machine;
mod sandbox;
pub mod values;
mod wait;

pub use beadroll_core::json::Value as Json;
pub use gate::{Gate, beside};
pub use isolation::assert_git_isolation;
pub use machine::{Invocation, Machine, Run, UNPROFILED, code};
pub use sandbox::{Sandbox, UNDER_TEST, built_bead, git, git_command, select_bead};
pub use wait::{REMOTE_SETTLES, wait_until, within};

/// Reads bead's output as a JSON document.
pub fn parse_json(text: &str) -> Result<Json, String> {
    beadroll_core::json::from_text(text).map_err(|error| error.to_string())
}
