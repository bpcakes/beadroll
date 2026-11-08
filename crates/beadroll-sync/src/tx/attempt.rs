//! A scoped writer puts every emitted event in the transaction's ownership attempt. Borrowing
//! the transaction prevents ordinary emission until this writer is released; no flag is toggled.

use beadroll_core::json::{Object, Value};
use beadroll_core::state::State;

use super::{Emitter, Tx};
use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    Journal,
    Attempt,
}

/// Events written through this scope belong to the same atomic attempt as its claim or release.
/// Ending the borrow leaves the transaction's ordinary emission mode unchanged.
pub struct Attempt<'t, 'w> {
    tx: &'t mut Tx<'w>,
}

impl<'w> Tx<'w> {
    /// Borrows a writer that bundles all its events into this transaction's ownership attempt.
    pub fn attempt(&mut self) -> Attempt<'_, 'w> {
        Attempt { tx: self }
    }
}

impl Attempt<'_, '_> {
    /// The current state, including earlier events of the transaction.
    pub fn state(&mut self) -> Result<&State, Error> {
        self.tx.state()
    }

    /// Lends the current state beside an emitter restricted to the ownership attempt.
    pub fn lend(&mut self) -> Result<(&State, Emitter<'_>), Error> {
        let (state, mut emitter) = self.tx.lend()?;
        emitter.route = Route::Attempt;
        Ok((state, emitter))
    }

    /// Emits into the ownership attempt, including ordinary edits such as `set` and `label_add`.
    pub fn emit(
        &mut self,
        op: &str,
        issue: Option<&str>,
        data: Object,
        event_id: Option<String>,
    ) -> Result<Value, Error> {
        let mut emitter = self.tx.emitter();
        emitter.route = Route::Attempt;
        emitter.emit(op, issue, data, event_id)
    }
}
