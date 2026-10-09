//! Local transactions (Python's `Tx`, `locked` and `writing`): one locked read-modify-append on
//! this machine's own journal.

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use beadroll_core::event::{FORMAT, MAX_NESTING, event_problem, payload_problem, write_problem};
use beadroll_core::journal::Journal;
use beadroll_core::json::{Object, Value};
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_git::IoError;
use beadroll_store::{
    Workspace, append_events, attempt_path, capture_where, encode_events, glob_ending, json_text,
    load_journal, private_dir, private_dirs, replica_id, uuid7, write_attempt_file,
};

use crate::error::Error;

/// Ownership events: settled with the remote, and kept one attempt per file.
pub const OWNERSHIP: [&str; 2] = ["claim", "release"];

/// One locked read-modify-append on this machine's own journal. Sequential events go to the
/// journal. Ownership events (`claim`, `release`) form an attempt that is settled with the remote
/// as one unit, and so does anything emitted after them with `conditional` set: an edit made
/// together with a claim lands with the claim or not at all.
#[derive(Debug)]
pub struct Tx<'a> {
    /// The workspace written to.
    pub ws: &'a Workspace,
    /// Who writes.
    pub actor: Value,
    /// This machine's id.
    pub replica: String,
    /// Every event the store holds, and those emitted so far.
    pub journal: Journal,
    /// The checkout's branch and commit, recorded on each event.
    pub place: Option<Value>,
    /// Sequential events emitted, to append.
    pub pending: Vec<Value>,
    /// The attempt's events emitted, to settle with the remote.
    pub ownership: Vec<Value>,
    /// Whether later events belong to the attempt.
    pub conditional: bool,
    state: Option<State>,
}

impl<'a> Tx<'a> {
    /// A transaction of `actor` on `ws`: this machine's identity (minted on first use), the store's
    /// journal, and where the caller works.
    pub fn new(ws: &'a Workspace, actor: Value) -> Result<Tx<'a>, Error> {
        let replica = replica_id(&ws.settings, true)?.unwrap_or_default();
        let journal = load_journal(ws)?;
        let place = capture_where(ws)?;
        Ok(Tx {
            ws,
            actor,
            replica,
            journal,
            place,
            pending: Vec::new(),
            ownership: Vec::new(),
            conditional: false,
            state: None,
        })
    }

    /// The state the journal replays into, with every event emitted so far.
    pub fn state(&mut self) -> Result<&State, Error> {
        if self.state.is_none() {
            self.state = Some(replay(self.journal.clone())?);
        }
        match &self.state {
            Some(state) => Ok(state),
            None => Err(Error::Validation("no state".to_owned())),
        }
    }

    /// Emits one event: `op` on `issue` (the event itself when `None`) with `data`, under
    /// `event_id` or a fresh one. It is checked by the rules replay applies, before it is minted,
    /// and must nest at most [`MAX_NESTING`] levels so that every supported Python can read it back.
    pub fn emit(
        &mut self,
        op: &str,
        issue: Option<&str>,
        data: Object,
        event_id: Option<String>,
    ) -> Result<Value, Error> {
        let id = event_id.unwrap_or_else(|| uuid7(&self.ws.settings.clock));
        let deps: Vec<Value> = self
            .journal
            .heads()
            .into_iter()
            .map(|dep| Value::from(dep.to_string()))
            .collect();
        let mut event = Object::new();
        event.insert("v".into(), Value::from(FORMAT));
        event.insert("id".into(), Value::from(id.as_str()));
        event.insert("replica".into(), Value::from(self.replica.as_str()));
        event.insert("deps".into(), Value::Array(deps));
        event.insert("actor".into(), self.actor.clone());
        event.insert("op".into(), Value::from(op));
        let issue = issue.filter(|issue| !issue.is_empty()).unwrap_or(&id);
        event.insert("issue".into(), Value::from(issue));
        event.insert("data".into(), Value::Object(data));
        if let Some(place) = &self.place {
            event.insert("where".into(), place.clone());
        }
        let event = Value::Object(event);
        let problem = event_problem(&event)
            .or_else(|| payload_problem(&event))
            .or_else(|| write_problem(&event));
        if let Some(problem) = problem {
            return Err(Error::Validation(if problem.contains("timestamp") {
                "the clock is beyond the year 2100; fix it before writing".to_owned()
            } else {
                problem
            }));
        }
        if event.deeper_than(MAX_NESTING) {
            return Err(Error::Validation(format!(
                "data nests more than {MAX_NESTING} levels deep"
            )));
        }
        encode_events(std::slice::from_ref(&event))?;
        if OWNERSHIP.contains(&op) || self.conditional {
            self.ownership.push(event.clone());
            // later emits build on it
            self.journal.add(event.clone(), &self.replica, "", false);
        } else {
            self.journal.add(event.clone(), &self.replica, "", true);
            self.pending.push(event.clone());
        }
        self.state = None;
        Ok(event)
    }
}

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
        write_attempt_file(&attempt_path(ws, events), events)?;
    }
    Ok(())
}

/// A local write: `body` emits into a transaction, under the store lock; then the sequential
/// events are appended and ownership events made here (offline, or with no remote) count at once.
/// A command that went through retires its actor's receipts: a repeat of an earlier command is a
/// new command. Nothing is written if `body` fails.
pub fn writing<T, E: From<Error>>(
    ws: &Workspace,
    actor: &Value,
    body: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
) -> Result<T, E> {
    locked(ws, || {
        let mut tx = Tx::new(ws, actor.clone())?;
        let replica = tx.replica.clone();
        if tx.state()?.chain_breaks().contains_key(&replica) {
            return Err(Error::JournalFork(
                "this machine's journal no longer forms one chain: the same machine identity has \
                 written from two places. Give one of them a new identity."
                    .to_owned(),
            )
            .into());
        }
        let result = body(&mut tx)?;
        let finish = || -> Result<(), Error> {
            if !tx.pending.is_empty() {
                append_events(ws, &tx.replica, &tx.pending)?;
            }
            if !tx.ownership.is_empty() {
                confirm_ownership(ws, &tx.ownership)?;
            }
            if !tx.pending.is_empty() || !tx.ownership.is_empty() {
                forget_receipts(ws, actor)?;
                ws.set_wrote(true);
            }
            Ok(())
        };
        finish()?;
        Ok(result)
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

/// Removes `path`; already gone is fine (Python's `unlink(missing_ok=True)`).
pub fn remove(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(IoError::at(&error, path).into()),
    }
}
