//! The only code in `bead` that starts git: the runner (its own session, a timeout that kills the
//! process group or only the client, output captured to unlinked files, signal names), the
//! environments for the checkout and for `sync.git`, typed commands, the push-outcome and
//! local-damage classifiers, and the remote URL rules (`docs/RUST-PORT.md` §2.1, §2.3d, §2.3f).
//!
//! Slice 2a fills it; until then it is empty.
