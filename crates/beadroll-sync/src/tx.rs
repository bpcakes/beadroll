//! Local transactions: one locked read-modify-append over this machine's journal.

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use beadroll_core::event::{FORMAT, MAX_NESTING, event_problem, payload_problem, write_problem};
use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::projection::{Extended, Projection, Record};
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_git::IoError;
use beadroll_store::{
    Actor, Workspace, append_events, attempt_path, capture_where, encode_events, glob_ending,
    json_text, load_journal, private_dir, private_dirs, projection, read_texts, replica_id, uuid7,
    write_attempt_file,
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
    /// The store's projection, the events emitted applied to it up to `applied`; `None` once one
    /// was emitted that it could not take as replay would apply it (a takeover, a machine's first
    /// record): the state is then the journal replayed with them.
    projection: Option<Projection>,
    /// The frontier, with the events emitted so far: what a new event builds on.
    heads: Vec<EventId>,
    /// Every event emitted so far, as a journal records it.
    emitted: Vec<Record>,
    /// How many of them the projection has: the rest are applied when the state is asked for,
    /// together, as a command that emits many before it asks would have them.
    applied: usize,
    /// The checkout's branch and commit, recorded on each event.
    pub place: Option<Value>,
    /// The attempt's events emitted, to settle with the remote.
    pub ownership: Vec<Value>,
    /// Whether later events belong to the attempt.
    pub conditional: bool,
    /// Whether the actor's text can be written ([`Actor::writable`]).
    writable: bool,
    /// The state replayed, once the projection is gone.
    state: Option<State>,
    /// How many of the events emitted the state replayed has: it is replayed again once more are.
    replayed: usize,
}

impl<'a> Tx<'a> {
    /// A transaction of `actor` on `ws`: this machine's identity (minted on first use), the store's
    /// journal, and where the caller works.
    pub fn new(ws: &'a Workspace, actor: &Actor) -> Result<Tx<'a>, Error> {
        Tx::on(ws, actor, projection(ws)?)
    }

    /// [`Tx::new`] on `projection`, the store's as it is now, which the caller read under the
    /// store lock it still holds: a round that merged has it already (roll-lqjj.29).
    pub fn on(ws: &'a Workspace, actor: &Actor, projection: Projection) -> Result<Tx<'a>, Error> {
        let replica = replica_id(&ws.settings, true)?.unwrap_or_default();
        let place = capture_where(ws)?;
        Ok(Tx {
            ws,
            actor: actor.value.clone(),
            replica,
            heads: projection.heads().to_vec(),
            projection: Some(projection),
            emitted: Vec::new(),
            applied: 0,
            place,
            ownership: Vec::new(),
            conditional: false,
            writable: actor.writable,
            state: None,
            replayed: 0,
        })
    }

    /// The sequential events emitted, taken out of the transaction to be appended: the journal's
    /// records among those emitted, kept once and each let go of once encoded, so that a write of
    /// many (an import emits one or more for each of its export's records) never holds them twice
    /// (roll-lqjj.9). The transaction is done with once they are taken.
    pub(crate) fn take_pending(&mut self) -> impl Iterator<Item = Value> + use<> {
        (std::mem::take(&mut self.emitted).into_iter())
            .filter(|record| record.sequenced)
            .map(|record| record.value)
    }

    /// What the next event emitted builds on: the frontier, or the last event emitted.
    pub fn heads(&self) -> &[EventId] {
        &self.heads
    }

    /// Whether any sequential event was emitted.
    pub fn has_pending(&self) -> bool {
        self.emitted.iter().any(|record| record.sequenced)
    }

    /// The state the journal replays into, with every event emitted so far. Of its issues' texts,
    /// those of the issues the events emitted are about are in it (roll-lqjj.6); a command reads
    /// in any other it prints.
    pub fn state(&mut self) -> Result<&State, Error> {
        if self.applied < self.emitted.len()
            && let Some(mut projection) = self.projection.take()
        {
            let records = self
                .emitted
                .get(self.applied..)
                .unwrap_or_default()
                .to_vec();
            self.applied = self.emitted.len();
            let about = (records.iter())
                .filter_map(|record| record.value.get("issue").and_then(Value::as_str))
                .filter_map(EventId::parse);
            // texts that cannot be read in have the journal replayed
            if read_texts(self.ws, projection.state_mut(), about)
                && let Extended::Applied(projection) = projection.extend(records)?
            {
                self.projection = Some(*projection);
            }
        }
        if let Some(projection) = &self.projection {
            return Ok(projection.state());
        }
        if self.replayed < self.emitted.len() {
            self.state = None;
        }
        if self.state.is_none() {
            let mut journal = load_journal(self.ws)?;
            for record in &self.emitted {
                let Record {
                    value, sequenced, ..
                } = record;
                journal.add(value.clone(), &self.replica, "", *sequenced);
            }
            self.state = Some(replay(journal)?);
            self.replayed = self.emitted.len();
        }
        match &self.state {
            Some(state) => Ok(state),
            None => Err(Error::Validation("no state".to_owned())),
        }
    }

    /// Emits op on issue, defaulting issue to the event itself, with data and a supplied or fresh
    /// id ([`Emitter::emit`]).
    pub fn emit(
        &mut self,
        op: &str,
        issue: Option<&str>,
        data: Object,
        event_id: Option<String>,
    ) -> Result<Value, Error> {
        self.emitter().emit(op, issue, data, event_id)
    }

    /// What emits into this transaction, the state left as it is.
    fn emitter(&mut self) -> Emitter<'_> {
        Emitter {
            ws: self.ws,
            actor: &self.actor,
            replica: &self.replica,
            heads: &mut self.heads,
            emitted: &mut self.emitted,
            place: &self.place,
            ownership: &mut self.ownership,
            conditional: self.conditional,
            writable: self.writable,
        }
    }

    /// The state, as [`Tx::state`] gives it, lent beside an [`Emitter`] that emits into this
    /// transaction: what a command reads while it emits is the state as it was before, as a copy
    /// taken first would be, without one (roll-lqjj.29). The events emitted are in the state the
    /// next time it is asked for.
    pub fn lend(&mut self) -> Result<(&State, Emitter<'_>), Error> {
        self.state()?;
        let Tx {
            ws,
            actor,
            replica,
            projection,
            heads,
            emitted,
            place,
            ownership,
            conditional,
            writable,
            state,
            ..
        } = self;
        let state = match (projection, state) {
            (Some(projection), _) => projection.state(),
            (None, Some(state)) => state,
            (None, None) => return Err(Error::Validation("no state".to_owned())),
        };
        let emitter = Emitter {
            ws,
            actor,
            replica,
            heads,
            emitted,
            place,
            ownership,
            conditional: *conditional,
            writable: *writable,
        };
        Ok((state, emitter))
    }

    /// The state with every event emitted, taken out of the transaction, which is done with: what
    /// the store holds once they are written.
    pub fn into_state(mut self) -> Result<State, Error> {
        self.state()?;
        match (self.projection, self.state) {
            (Some(projection), _) => Ok(projection.into_state()),
            (None, Some(state)) => Ok(state),
            (None, None) => Err(Error::Validation("no state".to_owned())),
        }
    }
}

/// What emits into a [`Tx`] while its state is lent ([`Tx::lend`]).
#[derive(Debug)]
pub struct Emitter<'t> {
    ws: &'t Workspace,
    actor: &'t Value,
    replica: &'t str,
    heads: &'t mut Vec<EventId>,
    emitted: &'t mut Vec<Record>,
    place: &'t Option<Value>,
    ownership: &'t mut Vec<Value>,
    conditional: bool,
    writable: bool,
}

impl Emitter<'_> {
    /// Emits op on issue, defaulting issue to the event itself, with data and a supplied or fresh
    /// id. Validate before minting, including the [`MAX_NESTING`] write limit.
    pub fn emit(
        &mut self,
        op: &str,
        issue: Option<&str>,
        data: Object,
        event_id: Option<String>,
    ) -> Result<Value, Error> {
        let id = event_id.unwrap_or_else(|| uuid7(&self.ws.settings.clock));
        let deps: Vec<Value> = (self.heads.iter())
            .map(|dep| Value::from(dep.to_string()))
            .collect();
        let mut event = Object::new();
        event.insert("v".into(), Value::from(FORMAT));
        event.insert("id".into(), Value::from(id.as_str()));
        event.insert("replica".into(), Value::from(self.replica));
        event.insert("deps".into(), Value::Array(deps));
        event.insert("actor".into(), self.actor.clone());
        event.insert("op".into(), Value::from(op));
        let issue = issue.filter(|issue| !issue.is_empty()).unwrap_or(&id);
        event.insert("issue".into(), Value::from(issue));
        event.insert("data".into(), Value::from(data));
        if let Some(place) = self.place {
            event.insert("where".into(), place.clone());
        }
        let event = Value::from(event);
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
        if !self.writable {
            // Invalid UTF-8 actor text cannot be encoded in an event.
            return Err(Error::Validation("text is not valid UTF-8".to_owned()));
        }
        let sequenced = !(OWNERSHIP.contains(&op) || self.conditional);
        if !sequenced {
            self.ownership.push(event.clone());
        }
        // later emits build on it
        *self.heads = EventId::parse(&id).into_iter().collect();
        self.emitted.push(Record {
            value: event.clone(),
            replica: self.replica.to_owned(),
            origin: String::new(),
            sequenced,
        });
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
        write_attempt_file(ws, &attempt_path(ws, events), events)?;
    }
    Ok(())
}

/// A local write: `body` emits into a transaction, under the store lock; then the sequential
/// events are appended and ownership events made here (offline, or with no remote) count at once.
/// A command that went through retires its actor's receipts: a repeat of an earlier command is a
/// new command. Nothing is written if `body` fails. The transaction, which holds the store's whole
/// projection, is let go of once the lock is: freeing it is no part of the write, every command
/// waiting for the lock would wait for it too, and on a large tracker it takes about as long as
/// decoding the projection did (83 ms and 136 ms at 200,000 events; roll-lqjj.32, X10).
pub fn writing<T, E: From<Error>>(
    ws: &Workspace,
    actor: &Actor,
    body: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
) -> Result<T, E> {
    written(ws, actor, false, body).map(|(result, _)| result)
}

/// [`writing`], and the state it leaves the store's journal in: the transaction's, with every
/// event it emitted, so that a command that answers from it does not read the store again
/// (roll-lqjj.29). `None` when the transaction cannot have it; the store's is then read as before.
pub fn writing_state<T, E: From<Error>>(
    ws: &Workspace,
    actor: &Actor,
    body: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
) -> Result<(T, Option<State>), E> {
    written(ws, actor, true, body)
}

/// [`writing`], with the transaction's state at the end when `keep` asks for it. The transaction
/// is let go of, or made into that state, once the lock is (roll-lqjj.32).
fn written<T, E: From<Error>>(
    ws: &Workspace,
    actor: &Actor,
    keep: bool,
    body: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
) -> Result<(T, Option<State>), E> {
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
        let kept = keep && tx.state().is_ok();
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
        Ok((result, kept))
    });
    let (result, kept) = result?;
    let state = made.filter(|_| kept).and_then(|tx| tx.into_state().ok());
    Ok((result, state))
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
