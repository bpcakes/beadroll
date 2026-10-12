//! The pure core of `bead` (`docs/RUST-PORT.md` §2.1): everything that reads and derives a tracker,
//! with no I/O (no filesystem, processes, clock or environment), so pancake-studio can link it alone.
//! Its answers are Python bead's, proven differentially against CPython until the cutover removed
//! the Python implementation (`cargo xtask diff`, at `python-0.1.0`).
//!
//! - [`json`] and [`text`]: Python's `json` and string semantics where bead depends on them.
//! - [`id`], [`time`] and [`key`]: event ids, event times, and the keys bead mints and renames.
//! - [`event`]: what an event must look like, with Python's validation messages; [`op`]: what it
//!   asks, its operation with a typed payload, and [`op::PyInt`], an integer as Python holds one.
//! - [`journal`] and [`order`]: journal lines read and settled, and the causal order of events.
//! - [`replay`] and [`state`]: the events folded into issues, with every register, tag and claim;
//!   [`projection`]: a replayed state that the events after it can be applied to.
//! - [`query`] and [`view`]: conflicts, readiness, blockers, cycles, and how an issue is shown.
//! - [`export`] and [`import`]: Beads JSONL out, and the plan that brings it in.
//! - [`published`]: a published journal, its manifest checked.
//! - [`code`]: the error codes on the wire.

pub mod code;
pub mod event;
pub mod export;
pub mod id;
pub mod import;
pub mod journal;
pub mod json;
pub mod key;
pub mod op;
pub mod order;
pub mod projection;
pub mod published;
pub mod query;
pub mod replay;
pub mod state;
pub mod text;
pub mod time;
pub mod view;
