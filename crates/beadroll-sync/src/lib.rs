//! Sync and ownership protocols: local transactions, remote
//! fetch/merge/publish, maintenance, pending-attempt settlement, unpublished counts, receipts and
//! fences.
//!
//! Commands try up to [`ROUNDS`] rounds. Between rounds they wait outside the store lock for a
//! random duration up to `0.1 × 2^(attempt − 1)` seconds, capped at two seconds.

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
    Emitter, OWNERSHIP, Tx, confirm_ownership, drop_pending, forget_receipts, locked, remove,
    writing, writing_state,
};
