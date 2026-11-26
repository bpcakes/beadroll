//! A transaction has one materialized view. Once incremental application cannot reproduce replay,
//! it stays on replay; a failed read or application leaves it ready to retry from the journal.

use beadroll_core::journal::JournalOrder;
use beadroll_core::projection::{Extended, Projection, Record};
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_store::{Workspace, extend_projection, load_journal};

use crate::error::Error;

#[derive(Debug, Default)]
pub(super) enum Materialized {
    Projected {
        projection: Box<Projection>,
        applied: usize,
        // Dropped after the actual view; present only in unit tests of lock lifetimes.
        #[cfg(test)]
        drop_probe: Option<super::tests::DropProbe>,
    },
    #[default]
    ReplayNeeded,
    Replayed {
        state: Box<State>,
        applied: usize,
        #[cfg(test)]
        drop_probe: Option<super::tests::DropProbe>,
    },
}

impl Materialized {
    pub(super) fn projected(projection: Projection) -> Self {
        Self::Projected {
            projection: Box::new(projection),
            applied: 0,
            #[cfg(test)]
            drop_probe: None,
        }
    }

    /// How many of the transaction's emitted events its state applies: none where there is none.
    pub(super) fn applied(&self) -> usize {
        match self {
            Self::Projected { applied, .. } | Self::Replayed { applied, .. } => *applied,
            Self::ReplayNeeded => 0,
        }
    }

    /// Takes the projection and its applied prefix only if it can accept more events.
    /// A failed extension or journal read must leave the next request ready to replay.
    fn take_extendable(&mut self) -> Option<(Box<Projection>, usize)> {
        match std::mem::take(self) {
            Self::Projected {
                projection,
                applied,
                ..
            } if projection.extendable() => Some((projection, applied)),
            _ => None,
        }
    }

    pub(super) fn ensure(
        &mut self,
        ws: &Workspace,
        replica: &str,
        emitted: &[Record],
    ) -> Result<(), Error> {
        match self {
            Self::Projected { applied, .. } | Self::Replayed { applied, .. }
                if *applied >= emitted.len() =>
            {
                return Ok(());
            }
            _ => {}
        }
        if let Some((projection, applied)) = self.take_extendable() {
            let records = emitted.get(applied..).unwrap_or_default().to_vec();
            if let Extended::Applied(projection) = extend_projection(ws, *projection, records)? {
                *self = Self::Projected {
                    projection,
                    applied: emitted.len(),
                    #[cfg(test)]
                    drop_probe: None,
                };
                return Ok(());
            }
        }
        let mut journal = load_journal(ws)?;
        for record in emitted {
            journal.add(
                record.value.clone(),
                replica,
                "",
                if record.sequenced {
                    JournalOrder::Sequenced
                } else {
                    JournalOrder::Attempt
                },
            );
        }
        *self = Self::Replayed {
            state: Box::new(replay(journal)?),
            applied: emitted.len(),
            #[cfg(test)]
            drop_probe: None,
        };
        Ok(())
    }

    pub(super) fn state(&self) -> Result<&State, Error> {
        match self {
            Self::Projected { projection, .. } => Ok(projection.state()),
            Self::Replayed { state, .. } => Ok(state),
            Self::ReplayNeeded => Err(Error::Validation("no state".to_owned())),
        }
    }

    pub(super) fn into_state(self) -> Result<State, Error> {
        match self {
            Self::Projected { projection, .. } => Ok(projection.into_state()),
            Self::Replayed { state, .. } => Ok(*state),
            Self::ReplayNeeded => Err(Error::Validation("no state".to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beadroll_core::projection::project;
    use beadroll_test_support::events::{Event, id, journal};

    /// A transaction with new records chooses replay before preparing its batch when the
    /// projection is waiting for a cause (AU03).
    #[test]
    fn a_projection_waiting_for_a_cause_is_set_aside_before_batch_preparation() {
        let cause = Event::new(&id(1), "init")
            .data(beadroll_test_support::json!({"prefix": "ex"}))
            .json();
        let waiting = Event::new(&id(2), "create")
            .deps(&[&id(1)])
            .data(beadroll_test_support::json!({"key": "ex-one", "fields": {}}))
            .json();
        let projection = project(journal(std::slice::from_ref(&waiting))).unwrap();
        assert_eq!(projection.state().pending().len(), 1);
        let mut view = Materialized::projected(projection);
        assert!(view.take_extendable().is_none());
        assert!(matches!(view, Materialized::ReplayNeeded));

        let mut view = Materialized::projected(project(journal(&[cause, waiting])).unwrap());
        assert!(view.take_extendable().is_some());
    }
}
