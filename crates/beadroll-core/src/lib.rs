//! The pure core of bead: tracker reading and derivation without filesystem access, processes,
//! clock or environment.
//!
//! - [`json`] and [`text`]: stored-data and output contracts.
//! - [`id`], [`time`] and [`key`]: event ids, timestamps and issue keys.
//! - [`event`] and [`op`]: validation and typed operations, including [`op::IntegerField`].
//! - [`journal`] and [`order`]: journal settling and causal order.
//! - [`replay`], [`state`] and [`projection`]: current state and incremental replay.
//! - [`query`] and [`view`]: readiness, conflicts, cycles and issue presentation.
//! - [`export`] and [`import`]: Beads JSONL conversion.
//! - [`published`]: published trees and manifest validation.
//! - [`code`]: wire error codes.

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
