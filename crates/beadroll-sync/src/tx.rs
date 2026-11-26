//! Local transactions: one locked read-modify-append over this machine's journal. The transaction
//! is here; what emits into it, and the judgement of a create's parent, in `tx/emitter.rs`; the
//! store lock and the writes that run a body in one, with what they hand back of it, in
//! `tx/writes.rs`.

mod attempt;
mod emitter;
mod heads;
mod materialized;
mod minted;
mod prepared;
mod writes;

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::DropProbe;

use std::collections::HashMap;

use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::op::OpName;
use beadroll_core::projection::{Projection, Record};
use beadroll_core::state::State;
use beadroll_store::{
    Actor, Sourced, Workspace, capture_where, ensure_replica_id, projection, uuid7,
};

use crate::error::Error;
pub use attempt::Attempt;
use attempt::Route;
pub use emitter::Emitter;
pub(crate) use emitter::{Made, names_parent, parent_problem};
use heads::Heads;
use materialized::Materialized;
pub use minted::Minted;

pub(crate) use writes::confirm_ownership;
pub use writes::{drop_pending, locked, writing, writing_spent, writing_state};

/// Ownership events: settled with the remote, and kept one attempt per file.
pub(crate) const OWNERSHIP: [OpName; 2] = [OpName::Claim, OpName::Release];

/// One locked read-modify-append on this machine's own journal. Sequential events go to the
/// journal. Ownership events (`claim`, `release`) form an attempt that is settled with the remote
/// as one unit, and so does anything emitted through [`Tx::attempt`]: an edit made
/// together with a claim lands with the claim or not at all. Every event of the journal comes
/// before the attempt's first: one after it is refused, the journal appended before the attempt is
/// published.
#[derive(Debug)]
pub struct Tx<'a> {
    /// The workspace written to: what the causes below were read from.
    ws: &'a Workspace,
    /// Who writes: fixed once it is begun, as whether its text can be written is judged then
    /// (`writable`).
    actor: Value,
    /// This machine's id: the journal written to, whose last record the causes below were chosen
    /// for, fixed once they are ([`Tx::replica`]).
    replica: String,
    /// The projection or replay, with the prefix of emitted events it contains.
    materialized: Materialized,
    /// The frontier, with the events emitted so far, by route: what a new event builds on.
    heads: Heads,
    /// Whether a cause an event emitted built on waits for one of its own: replay applies none of
    /// the events emitted after it until it comes, and a create among them is no parent.
    waits: bool,
    /// Whether this machine's last applicable event broke its journal chain: replay sets aside every
    /// record of its journal emitted, a create among them no parent, and applies an attempt's
    /// events, in no journal's chain (`writing` refuses a journal with a break before its body
    /// runs). A placeholder that broke the chain is no predecessor, and breaks nothing after it.
    broken: bool,
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
    place: Option<Value>,
    /// The attempt's events emitted, to settle with the remote: those a create among them was
    /// judged a parent by, written as they are ([`Tx::ownership`]).
    ownership: Vec<Value>,
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
            heads: Heads::new(causes.ids),
            waits: causes.waits,
            broken: causes.broken,
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
            .ensure(self.ws, &self.replica, self.emitted.iter())?;
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

    /// This machine's id: the journal the transaction writes to. Fixed once it is begun, as what
    /// its events build on was chosen for that journal's last record:
    ///
    /// ```compile_fail
    /// fn elsewhere(tx: &mut beadroll_sync::Tx<'_>) {
    ///     tx.replica = "0000bb02".to_owned();
    /// }
    /// ```
    pub fn replica(&self) -> &str {
        &self.replica
    }

    /// The attempt's events emitted so far, to settle with the remote. Read only: a create among
    /// them a later event was judged to have for a parent is written as it was emitted:
    ///
    /// ```compile_fail
    /// fn unmake(tx: &mut beadroll_sync::Tx<'_>) {
    ///     tx.ownership.clear();
    /// }
    /// ```
    pub fn ownership(&self) -> &[Value] {
        &self.ownership
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
            broken: self.broken,
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
            broken,
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
            broken: *broken,
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
