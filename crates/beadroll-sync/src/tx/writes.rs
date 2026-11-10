//! The writes that go through a transaction: the store lock, a local write that appends what its
//! body emitted, and what each hands back of the transaction (nothing, its state, or what it held).

use std::path::Path;

use beadroll_core::json::Value;
use beadroll_core::state::State;
use beadroll_git::IoError;
use beadroll_store::PublicationState;
use beadroll_store::{Actor, Workspace, attempt_path, write_attempt_file};

use super::{Spent, Tx};
use crate::error::{Error, OWN_CHAIN};
use crate::receipts::forget_receipts;

/// Whether the transaction must apply its emitted events for its caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteResult {
    /// DiscardState.
    DiscardState,
    /// KeepState.
    KeepState,
}

/// Runs `body` holding the store lock (`<ws>/lock`, `flock(2)` exclusive), the store's directories
/// made private first.
pub fn locked<T, E: From<Error>>(
    ws: &Workspace,
    body: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let _lock = ws.lock().map_err(Error::from).map_err(E::from)?;
    body()
}

/// The attempt counts: written, as one file, where every machine reads it.
pub fn confirm_ownership(ws: &Workspace, events: &[Value]) -> Result<(), Error> {
    if !events.is_empty() {
        write_attempt_file(ws, &attempt_path(ws, events), events)?;
        ws.record_commit();
    }
    Ok(())
}

/// A local write: `body` emits into a transaction, under the store lock; then the sequential
/// events are appended and ownership events made here (offline, or with no remote) count at once.
/// A command that went through retires its actor's receipts: a repeat of an earlier command is a
/// new command. Nothing is written if `body` fails. The transaction, which holds the store's whole
/// projection, is let go of once the lock is: freeing it is no part of the write, every command
/// waiting for the lock would wait for it too, and on a large tracker it takes about as long as
/// decoding the projection did (83 ms and 136 ms at 200,000 events; roll-lqjj.32, X10). A caller
/// whose process ends with its command does not have to free it at all ([`writing_spent`]).
pub fn writing<T, E: From<Error>>(
    ws: &Workspace,
    actor: &Actor,
    body: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
) -> Result<T, E> {
    written(ws, actor, WriteResult::DiscardState, body).map(|done| done.result)
}

/// [`writing`], and the state it leaves the store's journal in: the transaction's, with every
/// event it emitted, so that a command that answers from it does not read the store again
/// (roll-lqjj.29). `None` when the transaction cannot have it; the store's is then read as before.
pub fn writing_state<T, E: From<Error>>(
    ws: &Workspace,
    actor: &Actor,
    body: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
) -> Result<(T, Option<State>), E> {
    written(ws, actor, WriteResult::KeepState, body).map(|done| {
        let state = done
            .applied
            .then(|| done.spent.materialized.into_state().ok())
            .flatten();
        (done.result, state)
    })
}

/// [`writing`], and what its transaction held, handed over instead of let go of: for a caller
/// whose process ends with its command, which then keeps it until the process exits rather than
/// freeing it (roll-lqjj.42). The caller lets go of it first when it goes on to read a state or to
/// publish, so that it does not hold two: that is the caller's to see to, nothing here does.
pub fn writing_spent<T, E: From<Error>>(
    ws: &Workspace,
    actor: &Actor,
    body: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
) -> Result<(T, Spent), E> {
    written(ws, actor, WriteResult::DiscardState, body).map(|done| (done.result, done.spent))
}

/// A write that went through: what its body returned, whether its retained view has every event
/// applied (when [`written`] was asked to), and that view, kept beyond the lock's lifetime.
struct Written<T> {
    result: T,
    applied: bool,
    spent: Spent,
}

/// [`writing`], with the transaction's state at the end when `keep` asks for it. Preparing consumes
/// the transaction. Its view and any unwritten events survive the lock on failure too
/// (roll-lqjj.32); successful writes return the view for extraction or retention.
fn written<T, E: From<Error>>(
    ws: &Workspace,
    actor: &Actor,
    keep: WriteResult,
    body: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
) -> Result<Written<T>, E> {
    let keep = matches!(keep, WriteResult::KeepState);
    let mut spent = Spent::default();
    let mut pending_write = None;
    let result = locked(ws, || -> Result<(T, bool), E> {
        let mut tx = Tx::new(ws, actor)?;
        let built = (|| {
            let replica = tx.replica.clone();
            if tx.state()?.chain_breaks().contains_key(&replica) {
                return Err(Error::JournalFork(OWN_CHAIN.to_owned()).into());
            }
            body(&mut tx)
        })();
        // the events emitted applied while the transaction still has them
        let applied = built.is_ok() && keep && tx.state().is_ok();
        let (prepared, retained) = tx.prepare();
        spent = retained;
        let prepared = pending_write.insert(prepared);
        // Even a failed body releases its large view only after the lock is gone.
        let result = built?;
        let mut finish = || -> Result<(), Error> {
            let sequenced = prepared.append(ws)?;
            if !prepared.ownership.is_empty() {
                confirm_ownership(ws, &prepared.ownership)?;
            }
            if sequenced || !prepared.ownership.is_empty() {
                forget_receipts(ws, &actor.value)?;
                ws.set_wrote(PublicationState::Unpublished);
            }
            Ok(())
        };
        finish()?;
        Ok((result, applied))
    });
    let (result, applied) = result?;
    Ok(Written {
        result,
        applied,
        spent,
    })
}

/// Removes these attempts' pending files and their receipts.
pub fn drop_pending(ws: &Workspace, attempts: &[Vec<Value>]) -> Result<(), Error> {
    for events in attempts {
        let Some(id) = events
            .first()
            .and_then(|e| e.get("id"))
            .map(beadroll_core::text::str_of)
        else {
            continue;
        };
        let folder = ws.dir.join("pending");
        remove(&folder.join(format!("{id}.json")))?;
        remove(&folder.join(format!("{id}.command")))?;
    }
    Ok(())
}

/// Removes path, succeeding if already absent.
pub fn remove(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(IoError::at(&error, path).into()),
    }
}
