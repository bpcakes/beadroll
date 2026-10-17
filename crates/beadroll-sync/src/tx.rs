//! Local transactions: one locked read-modify-append over this machine's journal. The transaction
//! and what it emits are here; the store lock and the writes that run a body in one, with what they
//! hand back of it, are in `tx/writes.rs`.

mod writes;

use beadroll_core::event::{FORMAT, MAX_NESTING, event_problem, payload_problem, write_problem};
use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::projection::{Extended, Projection, Record};
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_store::{
    Actor, Basis, Sourced, Workspace, after_write, append_events, capture_where, encode_events,
    load_journal, projection_sourced, read_texts, replica_id, uuid7, write_back,
};

use crate::error::Error;

pub use writes::{
    confirm_ownership, drop_pending, forget_receipts, locked, remove, writing, writing_spent,
    writing_state,
};

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
    /// What the projection was made from, as one written back names the journal's files
    /// (roll-lqjj.51); `None` when they were not all whole.
    basis: Option<Basis>,
    /// The ids of the sequential events taken out to be appended, in their order.
    appended: Vec<EventId>,
    /// Whether the projection had every event emitted applied when they were taken out.
    whole: bool,
    /// The journal's files as this write left them ([`Tx::seal`]): its projection is then written
    /// back as made from them.
    sealed: Option<Basis>,
}

impl<'a> Tx<'a> {
    /// A transaction of `actor` on `ws`: this machine's identity (minted on first use), the store's
    /// journal, and where the caller works.
    pub fn new(ws: &'a Workspace, actor: &Actor) -> Result<Tx<'a>, Error> {
        let (projection, basis) = projection_sourced(ws)?.into_parts();
        Tx::from(ws, actor, projection, basis)
    }

    /// [`Tx::new`] on the store's projection as the caller read it under the store lock it still
    /// holds: a round that merged has it already (roll-lqjj.29). That is the store's as it is now
    /// while nothing has written under `events/` since, which the projection's sources say: when
    /// the journal's files are no longer the ones it was made from, the store is read again, as
    /// `new` does (roll-lqjj.37).
    pub fn on(ws: &'a Workspace, actor: &Actor, read: Sourced) -> Result<Tx<'a>, Error> {
        match read.into_current(ws) {
            Some(read) => {
                let (projection, basis) = read.into_parts();
                Tx::from(ws, actor, projection, basis)
            }
            None => Tx::new(ws, actor),
        }
    }

    /// A transaction on `projection`, the store's as it is now, made from `basis`.
    fn from(
        ws: &'a Workspace,
        actor: &Actor,
        projection: Projection,
        basis: Option<Basis>,
    ) -> Result<Tx<'a>, Error> {
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
            basis,
            appended: Vec::new(),
            whole: false,
            sealed: None,
        })
    }

    /// The sequential events emitted, taken out of the transaction to be appended: the journal's
    /// records among those emitted, kept once and each let go of once encoded, so that a write of
    /// many (an import emits one or more for each of its export's records) never holds them twice
    /// (roll-lqjj.9). The transaction is done with once they are taken.
    pub(crate) fn take_pending(&mut self) -> impl Iterator<Item = Value> + use<> {
        self.whole = self.projection.is_some() && self.applied == self.emitted.len();
        self.appended = (self.emitted.iter())
            .filter(|record| record.sequenced)
            .filter_map(|record| record.value.get("id")?.as_str().and_then(EventId::parse))
            .collect();
        (std::mem::take(&mut self.emitted).into_iter())
            .filter(|record| record.sequenced)
            .map(|record| record.value)
    }

    /// Applies every event emitted to the projection while the transaction still has them, to
    /// write it back (roll-lqjj.51), and appends the sequential ones to this machine's journal:
    /// what an ownership round does under its lock before it settles its attempt.
    pub(crate) fn append(&mut self) -> Result<(), Error> {
        self.apply()?;
        if self.has_pending() {
            let pending = self.take_pending();
            append_events(self.ws, &self.replica, pending)?;
        }
        Ok(())
    }

    /// Whether any sequential event was emitted.
    pub fn has_pending(&self) -> bool {
        self.emitted.iter().any(|record| record.sequenced)
    }

    /// The state the journal replays into, with every event emitted so far. Of its issues' texts,
    /// those of the issues the events emitted can change are in it (roll-lqjj.6,
    /// `Projection::about`); a command reads in any other it prints.
    pub fn state(&mut self) -> Result<&State, Error> {
        self.apply()?;
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

    /// Applies to the projection the events emitted since it was last asked for, as its state is;
    /// whether it then has every one. Without one, or once it declines them, it has none: the
    /// state is the journal replayed with them.
    pub(crate) fn apply(&mut self) -> Result<bool, Error> {
        if self.applied < self.emitted.len()
            && let Some(mut projection) = self.projection.take()
        {
            let records = self
                .emitted
                .get(self.applied..)
                .unwrap_or_default()
                .to_vec();
            self.applied = self.emitted.len();
            let about = projection.about(&records);
            // texts that cannot be read in have the journal replayed
            if read_texts(self.ws, projection.state_mut(), about)
                && let Extended::Applied(projection) = projection.extend(records)?
            {
                self.projection = Some(*projection);
            }
        }
        Ok(self.projection.is_some())
    }

    /// Names the journal's files as this write left them, under the store lock it still holds,
    /// once its sequential events are appended and, when `confirmed`, its ownership events count
    /// here: its projection is then written back as made from them ([`Tx::write_back`]). It is not
    /// when the write wrote nothing, the projection does not have every event emitted, or the
    /// journal holds anything else than what it was made from and those events
    /// (`beadroll_store::after_write`).
    pub(crate) fn seal(&mut self, confirmed: bool) {
        let confirmed: &[Value] = if confirmed { &self.ownership } else { &[] };
        let unconfirmed = !self.ownership.is_empty() && confirmed.is_empty();
        let wrote = !self.appended.is_empty() || !confirmed.is_empty();
        self.sealed = (self.basis.take())
            .filter(|_| self.whole && wrote && !unconfirmed)
            .and_then(|basis| after_write(self.ws, &basis, &self.appended, confirmed));
    }

    /// Writes the projection back once the store lock is let go of, as [`Tx::seal`] named the
    /// journal's files, as a read writes back what it applied (roll-lqjj.51): the read after the
    /// write then finds it current. A cache: a write-back that cannot be made changes nothing the
    /// write did.
    pub(crate) fn write_back(&mut self) {
        if let (Some(basis), Some(projection)) = (self.sealed.take(), self.projection.as_mut()) {
            write_back(self.ws, basis, projection);
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

    /// What the transaction held that costs to free, taken out of it, which is done with, as it
    /// stands: the events it emitted are in it only if its state was asked for since.
    fn into_spent(self) -> Spent {
        Spent {
            projection: self.projection,
            state: self.state,
        }
    }
}

/// What a write's transaction held when the write was done: its projection, or the state replayed
/// once it had none, which is almost all of what the transaction holds. [`writing_spent`] hands it
/// over where [`writing`] lets go of it: freeing the projection of a large tracker takes about as
/// long as decoding the state of one (83 ms at 200,000 events), and a caller whose process ends
/// with its command, as `bead` itself does, need not free it at all (roll-lqjj.42). A caller that
/// does not know that drops it, which frees it.
#[derive(Debug, Default)]
pub struct Spent {
    /// The store's projection, the events applied that the transaction asked its state for.
    projection: Option<Projection>,
    /// The journal replayed with the events emitted, once the projection was gone.
    state: Option<State>,
}

impl Spent {
    /// Whether it holds nothing: the transaction had let go of its projection and had not
    /// replayed the journal.
    pub fn is_empty(&self) -> bool {
        self.projection.is_none() && self.state.is_none()
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
