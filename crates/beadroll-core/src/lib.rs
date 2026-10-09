//! The pure core of `bead`: Python-compatible JSON and text semantics, event ids and times, event
//! validation, journal settling, causal order, replay into state, readiness and the other queries,
//! issue views, Beads import planning and export, key minting, and reading a published tree
//! (`docs/RUST-PORT.md` §2.1).
//!
//! It does no I/O: no filesystem, processes, clock or environment, so pancake-studio can link it
//! alone. Phase 1 fills it; until then it is empty.
