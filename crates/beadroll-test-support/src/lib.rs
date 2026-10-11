//! The Rust `tests/support.py`: a [`Sandbox`] (one local bare remote, `origin.git`, whose `main`
//! holds one commit) and [`Machine`]s (each with its own home, clone and identity) for black-box
//! tests of `bead`, plus [`git`] and [`code`]. Nothing outside the sandbox is read or written.
//!
//! Which `bead` a machine runs is always explicit (`docs/RUST-PORT.md` §4.4): the machine's own
//! override ([`Machine::with_bead`]), else `BEAD_UNDER_TEST`, else the default the consuming test
//! binary hands to [`Sandbox::new`]. In `crates/beadroll/tests/cli/main.rs` that default is
//! `env!("CARGO_BIN_EXE_bead")`; this crate never expands the macro, because Cargo defines it only
//! for the integration tests of the package that builds `bead`. The tests of the other crates hand
//! it [`built_bead`], the `bead` Cargo built beside them. A sandbox whose `bead` does not exist
//! fails its test at once and says how to build it.
//!
//! [`Json`] is beadroll-core's Python-compatible JSON, which reads what `bead --json` prints; the
//! [`json!`] macro and [`events`] build the events core's integration tests replay.
#![allow(clippy::unwrap_used, reason = "a failed fixture is a failed test")]
#![allow(clippy::expect_used, reason = "a failed fixture is a failed test")]
#![allow(clippy::panic, reason = "a failed fixture is a failed test")]
#![allow(clippy::print_stderr, reason = "a failed fixture is a failed test")]

pub mod events;
mod machine;
mod sandbox;

pub use beadroll_core::json::Value as Json;
pub use machine::{Invocation, Machine, Run, code};
pub use sandbox::{Sandbox, UNDER_TEST, built_bead, git, select_bead};

/// `text` read as a JSON document, as `json.loads` reads bead's output.
pub fn parse_json(text: &str) -> Result<Json, String> {
    beadroll_core::json::from_text(text).map_err(|error| error.to_string())
}
