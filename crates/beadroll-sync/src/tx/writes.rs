//! The writes that go through a transaction: the store lock, a local write that appends what its
//! body emitted, and what each hands back of the transaction (nothing, its state, or what it held).

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use beadroll_core::json::Value;
use beadroll_core::state::State;
use beadroll_git::IoError;
use beadroll_store::{
    Actor, Workspace, append_events, attempt_path, glob_ending, json_text, private_dir,
    private_dirs, write_attempt_file,
};

use super::{Spent, Tx};
use crate::error::Error;

/// Runs `body` holding the store lock (`<ws>/lock`, `flock(2)` exclusive), the store's directories
/// made private first.
pub fn locked<T, E: From<Error>>(
    ws: &Workspace,
    body: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let lock = take_lock(ws).map_err(E::from)?;
    ws.set_holding(true);
    let result = body();
    ws.set_holding(false);
    drop(lock);
    result
}

/// The store's lock file, held exclusively, the store's directories made private first.
fn take_lock(ws: &Workspace) -> Result<std::fs::File, Error> {
    private_dir(&ws.settings.home()?)?;
    private_dir(&ws.dir)?;
    private_dirs(&ws.events_dir)?;
    let path = ws.dir.join("lock");
    let lock = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| IoError::at(&e, &path))?;
    lock.lock().map_err(|e| IoError::at(&e, &path))?;
    Ok(lock)
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
    written(ws, actor, false, body).map(|done| done.result)
}

/// [`writing`], and the state it leaves the store's journal in: the transaction's, with every
/// event it emitted, so that a command that answers from it does not read the store again
/// (roll-lqjj.29). `None` when the transaction cannot have it; the store's is then read as before.
pub fn writing_state<T, E: From<Error>>(
    ws: &Workspace,
    actor: &Actor,
    body: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
) -> Result<(T, Option<State>), E> {
    written(ws, actor, true, body).map(|done| {
        let state = done
            .tx
            .filter(|_| done.applied)
            .and_then(|tx| tx.into_state().ok());
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
    written(ws, actor, false, body).map(|done| {
        let spent = done.tx.map_or_else(Spent::default, Tx::into_spent);
        (done.result, spent)
    })
}

/// A write that went through: what its body returned, whether the transaction has the events it
/// emitted applied (when [`written`] was asked to), and the transaction, which the lock no longer
/// protects and no one has let go of yet.
struct Written<'w, T> {
    result: T,
    applied: bool,
    tx: Option<Tx<'w>>,
}

/// [`writing`], with the transaction's state at the end when `keep` asks for it. The transaction
/// is given back with the result, to be let go of, or made into that state, once the lock is
/// (roll-lqjj.32); it is let go of here when the write fails.
fn written<'w, T, E: From<Error>>(
    ws: &'w Workspace,
    actor: &Actor,
    keep: bool,
    body: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
) -> Result<Written<'w, T>, E> {
    let mut made = None;
    let result = locked(ws, || -> Result<(T, bool), E> {
        let tx = made.insert(Tx::new(ws, actor)?);
        let replica = tx.replica.clone();
        if tx.state()?.chain_breaks().contains_key(&replica) {
            return Err(Error::JournalFork(
                "this machine's journal no longer forms one chain: the same machine identity has \
                 written from two places. Give one of them a new identity."
                    .to_owned(),
            )
            .into());
        }
        let result = body(tx)?;
        // the events emitted applied while the transaction still has them
        let applied = keep && tx.state().is_ok();
        let sequenced = tx.has_pending();
        let pending = tx.take_pending();
        let finish = || -> Result<(), Error> {
            if sequenced {
                append_events(ws, &tx.replica, pending)?;
            }
            if !tx.ownership.is_empty() {
                confirm_ownership(ws, &tx.ownership)?;
            }
            if sequenced || !tx.ownership.is_empty() {
                forget_receipts(ws, &actor.value)?;
                ws.set_wrote(true);
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
        tx: made,
    })
}

/// A later command of this actor went through: the receipts that would answer a repeat of an
/// earlier command go. Attempts still pending stay for sync to settle.
pub fn forget_receipts(ws: &Workspace, actor: &Value) -> Result<(), Error> {
    for receipt in glob_ending(&ws.dir.join("pending"), ".command") {
        let Ok(data) = std::fs::read(&receipt) else {
            continue;
        };
        let Ok(Value::Object(recorded)) = json_text(&data) else {
            continue;
        };
        if recorded.get("actor") == Some(actor) {
            remove(&receipt)?;
        }
    }
    Ok(())
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
