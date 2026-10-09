//! This machine's side of `bead`: `BEAD_HOME`, the store's files and modes, the lock, atomic
//! writes, journal segments and torn tails, attempt, pending and receipt files, identity and
//! aliases, workspace discovery and remote resolution, actor detection, `view`, and the failpoint
//! registry (`docs/RUST-PORT.md` §2.1).
//!
//! Slice 2a fills it; until then it is empty.
