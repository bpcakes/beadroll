//! The sync protocol of `bead`: transactions (`Tx`, `writing`), the link to the coordination remote
//! (fetch, merge, tree, publish, maintenance, rebuild), `synchronise`, the ownership protocol
//! (`coordinated`, receipts, fences, settling), `publish_soon` and the unpublished counts
//! (`docs/RUST-PORT.md` §2.1).
//!
//! Slices 2a and 2c fill it; until then it is empty.
