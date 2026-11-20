//! Local transactions: one locked read-modify-append over this machine's journal. The transaction
//! and what it emits are here; the store lock and the writes that run a body in one, with what they
//! hand back of it, are in `tx/writes.rs`.

mod attempt;
mod materialized;
mod minted;
mod prepared;
mod writes;

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::DropProbe;

use std::collections::HashMap;

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
pub use minted::Minted;

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
    /// Whether a cause an event emitted built on waits for one of its own: replay applies none of
    /// the events emitted after it until it comes, and a create among them is no parent.
    waits: bool,
    /// This machine's last applicable event, where the frontier does not reach it, until the
    /// transaction's first record of its journal names it
    /// ([`Causes`](beadroll_core::projection::Causes)).
    predecessor: Option<EventId>,
    /// Whether the predecessor waits for a cause of its own.
    predecessor_waits: bool,
    /// Every event emitted so far, as a journal records it.
    emitted: Vec<Record>,
    /// The creates among `emitted`, by id, with where each is among them: a create's parent while
    /// the state is of fewer events than make it.
    created: HashMap<EventId, usize>,
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
        // with this machine's last applicable event where the frontier does not reach it, which its
        // first record of the journal names: replay sets aside a record that does not build on it
        let causes = projection.causes_for(&replica);
        Ok(Tx {
            ws,
            actor: actor.value.clone(),
            replica,
            heads: causes.ids,
            waits: causes.waits,
            predecessor: causes.predecessor,
            predecessor_waits: causes.predecessor_waits,
            materialized: Materialized::projected(projection),
            emitted: Vec::new(),
            created: HashMap::new(),
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
    /// emitted since, the state made only where there is none. No event emitted since makes a
    /// parent of a pending create, its id [`Minted`] fresh, no cause any event waits for: what
    /// applying them adds to the issues is the creates among them, which `created` holds.
    fn judge_parent(&mut self, op: OpName, data: &Object) -> Result<(), Error> {
        if self.materialized.state().is_err() {
            self.state()?;
        }
        parent_problem(self.made(), &self.created, op, data)
    }

    /// The state as it stands, with how many of the events emitted it applies: none where there is
    /// none.
    fn made(&self) -> Made<'_> {
        Made {
            state: self.materialized.state().ok(),
            applied: self.materialized.applied(),
        }
    }

    /// A fresh id for the next event the caller emits ([`Minted`]).
    pub fn mint(&self) -> Minted {
        Minted(uuid7(&self.ws.settings.clock))
    }

    /// Emits op on issue, defaulting issue to the event itself, with data and a supplied or fresh
    /// id ([`Emitter::emit`]): the id of the event emitted.
    pub fn emit(
        &mut self,
        op: OpName,
        issue: Option<&str>,
        data: Object,
        event_id: Option<Minted>,
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
            waits: &mut self.waits,
            predecessor: &mut self.predecessor,
            predecessor_waits: self.predecessor_waits,
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
            waits,
            predecessor,
            predecessor_waits,
            place,
            ownership,
            writable,
            ..
        } = self;
        let state = materialized.state()?;
        let applied = materialized.applied();
        let emitter = Emitter {
            ws,
            actor,
            replica,
            heads,
            emitted,
            created,
            waits,
            predecessor,
            predecessor_waits: *predecessor_waits,
            place,
            ownership,
            route: Route::Journal,
            writable: *writable,
            known: Some(Made {
                state: Some(state),
                applied,
            }),
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
/// by the state lent and the creates emitted since, as [`Tx::emit`] does.
#[derive(Debug)]
pub struct Emitter<'t> {
    ws: &'t Workspace,
    actor: &'t Value,
    replica: &'t str,
    heads: &'t mut Vec<EventId>,
    emitted: &'t mut Vec<Record>,
    created: &'t mut HashMap<EventId, usize>,
    /// Whether replay applies nothing emitted from here until a cause comes ([`Tx`]'s `waits`).
    waits: &'t mut bool,
    /// The cause the transaction's first record of its journal names beside the frontier.
    predecessor: &'t mut Option<EventId>,
    /// Whether it waits for a cause of its own.
    predecessor_waits: bool,
    place: &'t Option<Value>,
    ownership: &'t mut Vec<Value>,
    route: Route,
    writable: bool,
    /// The state it was lent beside, which a create's parent must be an issue of; none for an
    /// emitter the transaction makes for one event, whose caller asked its own state first.
    known: Option<Made<'t>>,
}

/// A transaction's state, with how many of the events it emitted that state applies: a create
/// among them is a parent by the state once it is applied, set aside by replay or not, and by
/// [`Tx`]'s creates until then.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Made<'s> {
    state: Option<&'s State>,
    applied: usize,
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
    made: Made<'_>,
    created: &HashMap<EventId, usize>,
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
        (made.state).is_some_and(|st| st.issue(issue).is_some())
            || created.get(&issue).is_some_and(|at| *at >= made.applied)
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
    /// A fresh id for the next event the caller emits ([`Minted`]).
    pub fn mint(&self) -> Minted {
        Minted(uuid7(&self.ws.settings.clock))
    }

    /// Emits op on issue, defaulting issue to the event itself, with data and a supplied or fresh
    /// id: the id of the event emitted. Validate before minting, including the [`MAX_NESTING`] write
    /// limit.
    pub fn emit(
        &mut self,
        op: OpName,
        issue: Option<&str>,
        data: Object,
        event_id: Option<Minted>,
    ) -> Result<EventId, Error> {
        if let Some(known) = self.known
            && names_parent(op, &data)
        {
            parent_problem(known, self.created, op, &data)?;
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
        let sequenced = !OWNERSHIP.contains(&op) && self.route == Route::Journal;
        // this machine's last applicable event, where the frontier does not reach it, among the
        // causes of the transaction's first record of the journal: replay sets aside a record that
        // does not build on it. An attempt's event is in no journal's chain
        let predecessor =
            (*self.predecessor).filter(|predecessor| sequenced && parent != Some(*predecessor));
        let id = event_id.map_or_else(|| uuid7(&self.ws.settings.clock), |minted| minted.0);
        let deps: Vec<Value> = (self.heads.iter().chain(&predecessor).chain(&parent))
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
        // a parent by its record only where replay applies it as it comes: a create built on a
        // cause that waits makes no issue until then, as the state says once it is applied
        if sequenced && self.predecessor.take().is_some() {
            *self.waits |= self.predecessor_waits;
        }
        if op == OpName::Create && !*self.waits {
            self.created.insert(emitted, self.emitted.len() - 1);
        }
        Ok(emitted)
    }
}
