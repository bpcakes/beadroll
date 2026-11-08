//! Preparing consumes the emitter and separates durable work from the view retained after unlock.

use beadroll_core::json::Value;
use beadroll_core::projection::Record;
use beadroll_store::{Workspace, append_events};

use super::{Spent, Tx};
use crate::Error;

/// Events ready for persistence. There is no emitter or applied-prefix counter in this phase.
pub(crate) struct PreparedWrite {
    replica: String,
    pub(crate) ownership: Vec<Value>,
    emitted: Vec<Record>,
}

impl Tx<'_> {
    /// The caller retains the returned view outside the store lock, even if persistence fails.
    pub(crate) fn prepare(self) -> (PreparedWrite, Spent) {
        (
            PreparedWrite {
                replica: self.replica,
                ownership: self.ownership,
                emitted: self.emitted,
            },
            Spent {
                materialized: self.materialized,
            },
        )
    }
}

impl PreparedWrite {
    /// Append only the sequential records, before the attempt is confirmed or published.
    pub(crate) fn append(&mut self, ws: &Workspace) -> Result<bool, Error> {
        let sequenced = self.emitted.iter().any(|record| record.sequenced);
        let pending = self.take_pending();
        if sequenced {
            append_events(ws, &self.replica, pending)?;
        }
        Ok(sequenced)
    }

    /// Keep each record once and free it as encoded, including large imports (roll-lqjj.9).
    fn take_pending(&mut self) -> impl Iterator<Item = Value> + use<> {
        std::mem::take(&mut self.emitted)
            .into_iter()
            .filter(|record| record.sequenced)
            .map(|record| record.value)
    }
}
