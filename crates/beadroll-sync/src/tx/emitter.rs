//! What emits into a transaction ([`Emitter`]): every event's causes, its checks, and the record
//! it leaves; and the judgement of a create's parent by the state and the creates emitted since.

use std::collections::HashMap;

use beadroll_core::event::{FORMAT, MAX_NESTING, event_problem, payload_problem, write_problem};
use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::op::OpName;
use beadroll_core::projection::Record;
use beadroll_core::state::State;
use beadroll_store::{Workspace, encode_events, uuid7};

use super::attempt::Route;
use super::heads::Heads;
use super::{Minted, OWNERSHIP};
use crate::error::Error;

/// What emits into a [`Tx`](super::Tx) while its state is lent ([`Tx::lend`](super::Tx::lend)). It
/// judges a create's parent by the state lent and the creates emitted since, as
/// [`Tx::emit`](super::Tx::emit) does.
#[derive(Debug)]
pub struct Emitter<'t> {
    pub(super) ws: &'t Workspace,
    pub(super) actor: &'t Value,
    pub(super) replica: &'t str,
    pub(super) heads: &'t mut Heads,
    pub(super) emitted: &'t mut Vec<Record>,
    pub(super) created: &'t mut HashMap<EventId, usize>,
    /// Whether replay applies nothing emitted from here until a cause comes ([`Tx`](super::Tx)'s
    /// `waits`).
    pub(super) waits: &'t mut bool,
    /// The cause the transaction's first record of its journal names beside the frontier.
    pub(super) predecessor: &'t mut Option<EventId>,
    /// Whether it waits for a cause of its own.
    pub(super) predecessor_waits: bool,
    /// Whether the machine's journal chain is broken already ([`Tx`](super::Tx)'s `broken`).
    pub(super) broken: bool,
    pub(super) place: &'t Option<Value>,
    pub(super) ownership: &'t mut Vec<Value>,
    pub(super) route: Route,
    pub(super) writable: bool,
    /// The state it was lent beside, which a create's parent must be an issue of; none for an
    /// emitter the transaction makes for one event, whose caller asked its own state first.
    pub(super) known: Option<Made<'t>>,
}

/// A transaction's state, with how many of the events it emitted that state applies: a create
/// among them is a parent by the state once it is applied, set aside by replay or not, and by
/// [`Tx`](super::Tx)'s creates until then.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Made<'s> {
    pub(super) state: Option<&'s State>,
    pub(super) applied: usize,
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
    /// id: the id of the event emitted. Validate before minting, including the [`MAX_NESTING`]
    /// write limit.
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
        let sequenced = !OWNERSHIP.contains(&op) && self.route == Route::Journal;
        let parent = (names_parent(op, &data))
            .then(|| {
                data.get("parent")
                    .and_then(Value::as_str)
                    .and_then(EventId::parse)
            })
            .flatten();
        // the route the event takes: a claim or a release is an attempt's, wherever it is emitted
        let lane = if sequenced {
            Route::Journal
        } else {
            Route::Attempt
        };
        // every record of the journal before the attempt's first event ([`Heads`])
        if !self.heads.admits(lane) {
            return Err(Error::Validation(
                "a record of the journal must come before this command's ownership attempt: \
                 emit it through the attempt, or before it"
                    .to_owned(),
            ));
        }
        let heads = self.heads.ids();
        let parent = parent.filter(|parent| !heads.contains(parent));
        // this machine's last applicable event, where the frontier does not reach it, among the
        // causes of the transaction's first record of the journal: replay sets aside a record that
        // does not build on it. An attempt's event is in no journal's chain
        let predecessor =
            (*self.predecessor).filter(|predecessor| sequenced && parent != Some(*predecessor));
        let id = event_id.map_or_else(|| uuid7(&self.ws.settings.clock), |minted| minted.0);
        let deps: Vec<Value> = (heads.iter().chain(&predecessor).chain(&parent))
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
        self.heads.emitted(emitted, lane);
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
        if op == OpName::Create && !*self.waits && !(self.broken && sequenced) {
            self.created.insert(emitted, self.emitted.len() - 1);
        }
        Ok(emitted)
    }
}
