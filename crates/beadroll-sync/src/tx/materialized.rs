//! A transaction has one materialized view. Once incremental application cannot reproduce replay,
//! it stays on replay; a failed read or application leaves it ready to retry from the journal.

use beadroll_core::journal::JournalOrder;
use beadroll_core::projection::{Appended, Extended, Projection, Record};
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_store::{Workspace, extend_appended, load_journal};

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

    pub(super) fn ensure<'a>(
        &mut self,
        ws: &Workspace,
        replica: &str,
        emitted: impl ExactSizeIterator<Item = &'a Record> + Clone,
    ) -> Result<(), Error> {
        let count = emitted.len();
        match self {
            Self::Projected { applied, .. } | Self::Replayed { applied, .. }
                if *applied >= count =>
            {
                return Ok(());
            }
            _ => {}
        }
        // Leave the view ready to replay on error. Only an extendable projection needs a
        // batch of its own. Full replay reads the original records below.
        if let Self::Projected {
            projection,
            applied,
            ..
        } = std::mem::take(self)
            && projection.extendable()
        {
            let records = Appended::read_records(&mut emitted.clone().skip(applied).cloned());
            if let Extended::Applied(projection) = extend_appended(ws, *projection, records)? {
                *self = Self::Projected {
                    projection,
                    applied: count,
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
            applied: count,
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
