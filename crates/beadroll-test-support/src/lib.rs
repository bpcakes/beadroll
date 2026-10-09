//! The Rust `tests/support.py`: a [`Sandbox`] (one local bare remote, `origin.git`, whose `main`
//! holds one commit) and [`Machine`]s (each with its own home, clone and identity) for black-box
//! tests of `bead`, plus [`git`] and [`code`]. Nothing outside the sandbox is read or written.
//!
//! Which `bead` a machine runs is always explicit (`docs/RUST-PORT.md` §4.4): the machine's own
//! override ([`Machine::with_bead`]), else `BEAD_UNDER_TEST`, else the default the consuming test
//! binary hands to [`Sandbox::new`]. In `crates/beadroll/tests/cli/main.rs` that default is
//! `env!("CARGO_BIN_EXE_bead")`; this crate never expands the macro, because Cargo defines it only
//! for the integration tests of the package that builds `bead`. Phase 0 runs the black-box tests
//! against Python's `./bead` by naming it in `BEAD_UNDER_TEST`.
//!
//! [`Json`] reads what `bead --json` prints until `beadroll-core` has its Python-compatible JSON
//! (Phase 1), which replaces it.
#![allow(clippy::unwrap_used, reason = "a failed fixture is a failed test")]
#![allow(clippy::expect_used, reason = "a failed fixture is a failed test")]
#![allow(clippy::panic, reason = "a failed fixture is a failed test")]
#![allow(clippy::print_stderr, reason = "a failed fixture is a failed test")]

mod json;
mod machine;
mod sandbox;

pub use json::Json;
pub use machine::{Invocation, Machine, Run, code};
pub use sandbox::{Sandbox, UNDER_TEST, git, select_bead};
