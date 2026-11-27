//! A write that publishes itself before it answers: `create` of a child, and `update`, `close` and
//! `delete` ending a claim. Each chooses where the write goes before it lets go of what the write
//! held, as `cli::finish` does (roll-lqjj.50), and lets go of it only when a remote is to be
//! exchanged with; with nowhere to go, it gets it back to answer from and keep.

use std::ffi::OsStr;

use beadroll_store::Workspace;
use beadroll_sync::{NetworkMode, destination, publish_soon_at, publish_soon_to};

/// What came of a write's own publish ([`publish_first`]), and what the write held, when nothing
/// was exchanged.
pub(crate) enum Published<T> {
    /// Nothing went to a remote or came from one: `--offline`, a tracker with no remote, or a
    /// remote that could not be chosen, which `warning` says. The store holds what the write left
    /// it and no more, so `held` still says what the store holds, and is handed back.
    Unsent { held: T, warning: Option<String> },
    /// A remote was exchanged with, which may have brought in what other machines wrote, even when
    /// it failed (`warning`) after the fetch: the store is to be read again. What the write held
    /// was let go of before the exchange began, so that the exchange did not hold it beside the
    /// state it reads.
    Exchanged { warning: Option<String> },
}

impl<T> Published<T> {
    /// What the publish says when it failed: the local write stands, not yet published.
    pub(crate) fn warning(&self) -> Option<&str> {
        match self {
            Published::Unsent { warning, .. } | Published::Exchanged { warning } => {
                warning.as_deref()
            }
        }
    }
}

/// Publishes what was written in `ws` to the remote `network` and `remote` choose, having let go
/// of `held`, what the write held, first; or nothing, with `held` handed back, when no remote is
/// chosen. Taking `held` by value is what keeps it from being held through an exchange.
pub(crate) fn publish_first<T>(
    ws: &Workspace,
    network: NetworkMode,
    remote: Option<&OsStr>,
    held: T,
) -> Published<T> {
    match destination(ws, network, remote) {
        None => Published::Unsent {
            held,
            warning: None,
        },
        // why the remote could not be chosen, said as a failed publish says it
        Some(Err(error)) => Published::Unsent {
            held,
            warning: publish_soon_at(ws, Some(Err(error))),
        },
        Some(Ok(endpoint)) => {
            drop(held);
            Published::Exchanged {
                warning: publish_soon_to(ws, &endpoint),
            }
        }
    }
}

#[cfg(test)]
mod tests;
