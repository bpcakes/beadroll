//! The sync protocol of `bead` (`docs/RUST-PORT.md` §2.1): local transactions (`Tx`, `writing`),
//! the link to the coordination remote (fetch, merge, tree, publish with its four outcomes,
//! maintenance and rebuild), `synchronise` with its settling of this machine's pending attempts,
//! `publish_soon`, the unpublished counts, and the ownership protocol (`coordinated`, its
//! receipts and fences).
//!
//! The protocol constants are Python's: [`ROUNDS`] rounds, `U(0.1, 0.4) × attempt` seconds between
//! them, outside the store lock.

mod error;
mod link;
mod ownership;
mod synchronise;
mod tx;

pub use beadroll_git::PushOutcome;
pub use error::Error;
pub use link::Link;
pub use ownership::{Mode, Settled, coordinated, forget, settle_commands, settle_pending};
pub use synchronise::{
    ROUNDS, Synced, adopt_published, conflict_detail, pause, publish_soon, publish_soon_to,
    refused, synchronise, unpublished_count,
};
pub use tx::{
    OWNERSHIP, Tx, confirm_ownership, drop_pending, forget_receipts, locked, remove, writing,
};
