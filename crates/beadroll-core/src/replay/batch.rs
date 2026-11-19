//! Owns state while applying events. Temporary dependency holes never leave a batch.

use std::collections::HashSet;

use super::{Names, apply_one, finish, finish_some};
use crate::id::EventId;
use crate::journal::Rest;
use crate::state::{Integrity, KeySpace, Knowledge, State};

pub(crate) struct Batch {
    state: State,
}

impl Batch {
    pub(crate) fn new(state: State) -> Self {
        Self { state }
    }

    pub(crate) fn apply(
        &mut self,
        knowledge: &Knowledge,
        at: usize,
        event: &Rest,
        inert: Option<&String>,
        superseded: &HashSet<EventId>,
        names: &Names,
    ) -> Result<(), KeySpace> {
        apply_one(
            &mut self.state,
            knowledge,
            at,
            event,
            inert,
            superseded,
            names,
        )
    }

    /// Replay can retry exhausted takeovers without exposing a partially applied State.
    pub(super) fn integrity(&self) -> &[Integrity] {
        &self.state.integrity
    }

    pub(crate) fn finish(mut self, knowledge: Knowledge) -> State {
        self.state.knowledge = knowledge;
        finish(&mut self.state);
        self.state
    }

    /// Incremental application derives only what this batch can have changed.
    pub(crate) fn finish_some(mut self, knowledge: Knowledge, doing: &[Box<Rest>]) -> State {
        self.state.knowledge = knowledge;
        finish_some(&mut self.state, doing);
        self.state
    }
}
