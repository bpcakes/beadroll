//! Local transactions: one locked read-modify-append over this machine's journal. The transaction
//! and what it emits are here; the store lock and the writes that run a body in one, with what they
//! hand back of it, are in `tx/writes.rs`.

mod attempt;
mod materialized;
mod prepared;
mod writes;

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::DropProbe;

use std::collections::HashSet;

use beadroll_core::event::{FORMAT, MAX_NESTING, event_problem, payload_problem, write_problem};
use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::op::OpName;
use beadroll_core::projection::{Projection, Record};
use beadroll_core::state::State;
use beadroll_store::{
    Actor, Sourced, Workspace, capture_where, encode_events, ensure_replica_id, projection, uuid7,
};

use crate::error::Error;
pub use attempt::Attempt;
use attempt::Route;
use materialized::Materialized;

pub(crate) use writes::confirm_ownership;
pub use writes::{drop_pending, locked, writing, writing_spent, writing_state};

/// Ownership events: settled with the remote, and kept one attempt per file.
pub(crate) const OWNERSHIP: [OpName; 2] = [OpName::Claim, OpName::Release];

/// One locked read-modify-append on this machine's own journal. Sequential events go to the
/// journal. Ownership events (`claim`, `release`) form an attempt that is settled with the remote
/// as one unit, and so does anything emitted through [`Tx::attempt`]: an edit made
/// together with a claim lands with the claim or not at all.
#[derive(Debug)]
pub struct Tx<'a> {
    /// The workspace written to.
    pub ws: &'a Workspace,
    /// Who writes.
    pub actor: Value,
    /// This machine's id.
    pub replica: String,
    /// The projection or replay, with the prefix of emitted events it contains.
    materialized: Materialized,
    /// The frontier, with the events emitted so far: what a new event builds on.
    heads: Vec<EventId>,
    /// Every event emitted so far, as a journal records it.
    emitted: Vec<Record>,
    /// The creates among `emitted`, by id, which a create's parent may be.
    created: HashSet<EventId>,
    /// Every event among `emitted`, by id: a supplied id is no second event's.
    ids: HashSet<EventId>,
    /// The checkout's branch and commit, recorded on each event.
    pub place: Option<Value>,
    /// The attempt's events emitted, to settle with the remote.
    pub ownership: Vec<Value>,
    /// Whether the actor's text can be written ([`Actor::writable`]).
    writable: bool,
}

impl<'a> Tx<'a> {
    /// A transaction of `actor` on `ws`: this machine's identity (minted on first use), the store's
    /// journal, and where the caller works.
    pub fn new(ws: &'a Workspace, actor: &Actor) -> Result<Tx<'a>, Error> {
        Tx::from(ws, actor, projection(ws)?)
    }

    /// [`Tx::new`] on the store's projection as the caller read it under the store lock it still
    /// holds: a round that merged has it already (roll-lqjj.29). That is the store's as it is now
    /// while nothing has written under `events/` since, which the projection's sources say: when
    /// the journal's files are no longer the ones it was made from, the store is read again, as
    /// `new` does (roll-lqjj.37).
    pub fn on(ws: &'a Workspace, actor: &Actor, read: Sourced) -> Result<Tx<'a>, Error> {
        match read.into_current(ws) {
            Some(projection) => Tx::from(ws, actor, projection),
            None => Tx::new(ws, actor),
        }
    }

    /// A transaction on `projection`, the store's as it is now.
    fn from(ws: &'a Workspace, actor: &Actor, projection: Projection) -> Result<Tx<'a>, Error> {
        let replica = ensure_replica_id(&ws.settings)?;
        let place = capture_where(ws)?;
        Ok(Tx {
            ws,
            actor: actor.value.clone(),
            replica,
            heads: projection.heads().to_vec(),
            materialized: Materialized::projected(projection),
            emitted: Vec::new(),
            created: HashSet::new(),
            ids: HashSet::new(),
            place,
            ownership: Vec::new(),
            writable: actor.writable,
        })
    }

    /// The state the journal replays into, with every event emitted so far. Of its issues' texts,
    /// those of the issues the events emitted can change are in it (roll-lqjj.6,
    /// `Projection::about`); a command reads in any other it prints.
    pub fn state(&mut self) -> Result<&State, Error> {
        self.materialized
            .ensure(self.ws, &self.replica, &self.emitted)?;
        self.materialized.state()
    }

    /// [`parent_problem`] of a create naming a parent: by the state as it stands and the creates
    /// emitted since, and, for a parent neither holds, by the state with every event emitted since
    /// applied (a pending create a cause emitted since makes one), so that only a parent missing
    /// from both costs a pass over the issues.
    fn judge_parent(&mut self, op: OpName, data: &Object) -> Result<(), Error> {
        let stale = parent_problem(self.materialized.state().ok(), &self.created, op, data);
        if stale.is_ok() {
            return stale;
        }
        self.state()?;
        parent_problem(self.materialized.state().ok(), &self.created, op, data)
    }

    /// Emits op on issue, defaulting issue to the event itself, with data and a supplied or fresh
    /// id ([`Emitter::emit`]): the id of the event emitted.
    pub fn emit(
        &mut self,
        op: OpName,
        issue: Option<&str>,
        data: Object,
        event_id: Option<String>,
    ) -> Result<EventId, Error> {
        // the state is asked of only for a create naming a parent: any other write emits on it as
        // it did, read once
        if names_parent(op, &data) {
            self.judge_parent(op, &data)?;
        }
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
            created: &mut self.created,
            ids: &mut self.ids,
            place: &self.place,
            ownership: &mut self.ownership,
            route: Route::Journal,
            writable: self.writable,
            known: None,
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
            materialized,
            heads,
            emitted,
            created,
            ids,
            place,
            ownership,
            writable,
            ..
        } = self;
        let state = materialized.state()?;
        let emitter = Emitter {
            ws,
            actor,
            replica,
            heads,
            emitted,
            created,
            ids,
            place,
            ownership,
            route: Route::Journal,
            writable: *writable,
            known: Some(state),
        };
        Ok((state, emitter))
    }

    /// The state with every event emitted, taken out of the transaction, which is done with: what
    /// the store holds once they are written.
    pub fn into_state(mut self) -> Result<State, Error> {
        self.state()?;
        self.materialized.into_state()
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
    materialized: Materialized,
}

impl Spent {
    /// Keep a completed view alive without exposing it as a source for another operation.
    pub(crate) fn from_projection(projection: Projection) -> Self {
        Self {
            materialized: Materialized::projected(projection),
        }
    }

    /// Whether it holds nothing: the transaction had let go of its projection and had not
    /// replayed the journal.
    pub fn is_empty(&self) -> bool {
        matches!(self.materialized, Materialized::ReplayNeeded)
    }
}

/// What emits into a [`Tx`] while its state is lent ([`Tx::lend`]). It judges a create's parent
/// by the state lent and the creates emitted, the state borrowed for the lend: a parent a cause
/// emitted since has made replayable (a pending create) is judged once the lend is over, by
/// [`Tx::emit`].
#[derive(Debug)]
pub struct Emitter<'t> {
    ws: &'t Workspace,
    actor: &'t Value,
    replica: &'t str,
    heads: &'t mut Vec<EventId>,
    emitted: &'t mut Vec<Record>,
    created: &'t mut HashSet<EventId>,
    ids: &'t mut HashSet<EventId>,
    place: &'t Option<Value>,
    ownership: &'t mut Vec<Value>,
    route: Route,
    writable: bool,
    /// The state it was lent beside, which a create's parent must be an issue of; none for an
    /// emitter the transaction makes for one event, whose caller asked its own state first.
    known: Option<&'t State>,
}

/// Whether `op` with `data` is a create naming a parent, which [`parent_problem`] judges.
pub(crate) fn names_parent(op: OpName, data: &Object) -> bool {
    op == OpName::Create && data.contains_key("parent")
}

/// `VALIDATION` for a create naming as its parent (`parent`) what is neither an issue of `state`
/// nor a create among `created` (this transaction's): replay would find no parent to follow. Its
/// key is the caller's to mint from the parent's current key (`State::mint_key`, as `bead create`
/// does), of a shape `write_problem` judges: judging the key itself would need the parent's
/// settled key, which the state has only once every event emitted before is applied, a pass over
/// every issue a create (`docs/REVIEWING.md` §2).
pub(crate) fn parent_problem(
    state: Option<&State>,
    created: &HashSet<EventId>,
    op: OpName,
    data: &Object,
) -> Result<(), Error> {
    if op != OpName::Create {
        return Ok(());
    }
    let Some(parent) = data.get("parent") else {
        return Ok(());
    };
    let issue = (parent.as_str()).and_then(EventId::parse);
    let held = issue.is_some_and(|issue| {
        state.is_some_and(|st| st.issue(issue).is_some()) || created.contains(&issue)
    });
    if held {
        return Ok(());
    }
    let named = parent.as_str().unwrap_or("a value that is no id");
    Err(Error::Validation(format!(
        "a create's parent must be an issue of this tracker, not {named}"
    )))
}

impl Emitter<'_> {
    /// Emits op on issue, defaulting issue to the event itself, with data and a supplied or fresh
    /// id: the id of the event emitted. Validate before minting, including the [`MAX_NESTING`] write
    /// limit.
    pub fn emit(
        &mut self,
        op: OpName,
        issue: Option<&str>,
        data: Object,
        event_id: Option<String>,
    ) -> Result<EventId, Error> {
        if let Some(known) = self.known
            && names_parent(op, &data)
        {
            parent_problem(Some(known), self.created, op, &data)?;
        }
        // a create's parent among its causes, however it is emitted (siblings of one parent each
        // name it, the head before them being the sibling before): replay follows only a parent
        // named there. This event's alone: the heads change only once it is emitted
        let parent = (names_parent(op, &data))
            .then(|| {
                data.get("parent")
                    .and_then(Value::as_str)
                    .and_then(EventId::parse)
            })
            .flatten()
            .filter(|parent| !self.heads.contains(parent));
        // a supplied id is no event's this transaction emitted: the journal would set both aside,
        // and a create's id would stand for an issue it no longer makes
        if let Some(supplied) = event_id.as_deref().and_then(EventId::parse)
            && self.ids.contains(&supplied)
        {
            return Err(Error::Validation(format!(
                "{supplied} is an event this transaction emitted already"
            )));
        }
        let id = event_id.unwrap_or_else(|| uuid7(&self.ws.settings.clock));
        let deps: Vec<Value> = (self.heads.iter().chain(&parent))
            .map(|dep| Value::from(dep.to_string()))
            .collect();
        let mut event = Object::new();
        event.insert("v".into(), Value::from(FORMAT));
        event.insert("id".into(), Value::from(id.as_str()));
        event.insert("replica".into(), Value::from(self.replica));
        event.insert("deps".into(), Value::Array(deps));
        event.insert("actor".into(), self.actor.clone());
        event.insert("op".into(), Value::from(op.as_str()));
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
        let sequenced = !OWNERSHIP.contains(&op) && self.route == Route::Journal;
        if !sequenced {
            self.ownership.push(event.clone());
        }
        // the id passed validation above; later emits build on it
        let emitted = EventId::parse(&id)
            .ok_or_else(|| Error::Validation(format!("{id} is not an event id")))?;
        *self.heads = vec![emitted];
        self.emitted.push(Record {
            value: event,
            replica: self.replica.to_owned(),
            origin: String::new(),
            sequenced,
        });
        self.ids.insert(emitted);
        if op == OpName::Create {
            self.created.insert(emitted);
        }
        Ok(emitted)
    }
}
