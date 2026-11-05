//! A transaction has one materialized view. Once incremental application cannot reproduce replay,
//! it stays on replay; a failed read or application leaves it ready to retry from the journal.

use beadroll_core::journal::JournalOrder;
use beadroll_core::projection::{Extended, Projection, Record};
use beadroll_core::replay::replay;
use beadroll_core::state::State;
use beadroll_store::{Workspace, load_journal, read_texts};

use crate::error::Error;

#[derive(Debug, Default)]
pub(super) enum Materialized {
    Projected {
        projection: Box<Projection>,
        applied: usize,
    },
    #[default]
    ReplayNeeded,
    Replayed {
        state: Box<State>,
        applied: usize,
    },
}

impl Materialized {
    pub(super) fn projected(projection: Projection) -> Self {
        Self::Projected {
            projection: Box::new(projection),
            applied: 0,
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
        // Application consumes the projection. On any error the next request must replay,
        // rather than reuse a view that may contain only part of the emitted prefix.
        if let Self::Projected {
            mut projection,
            applied,
        } = std::mem::take(self)
        {
            let records = emitted.get(applied..).unwrap_or_default().to_vec();
            let about = projection.about(&records);
            if read_texts(ws, projection.state_mut(), about)
                && let Extended::Applied(projection) = projection.extend(records)?
            {
                *self = Self::Projected {
                    projection,
                    applied: emitted.len(),
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
