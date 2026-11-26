//! Preparation for applying an exact batch to a projection or requesting replay.

use crate::id::EventId;
use crate::state::{KeySpace, TextPlace};

use super::appended::{OwnedRecords, ReadableAppended};
use super::{Appended, Extended, Projection, Record, Unreadable};

/// An owned projection with the exact batch and text locations it can try to apply.
/// A projection that already requires replay discards a nonempty batch before conversion.
/// Obtain this from [`Projection::prepare_extension`] or [`Projection::prepare_appended`],
/// load [`Self::wanted_texts`], then consume it with [`Self::apply`].
///
/// ```compile_fail,E0382
/// use beadroll_core::projection::{Projection, Record};
/// fn change_batch(projection: Projection, mut records: Vec<Record>) {
///     let extension = projection.prepare_extension(records);
///     records.clear();
///     extension.apply();
/// }
/// ```
#[derive(Debug)]
#[must_use = "load the requested texts and apply, or discard the preparation and replay"]
pub struct Extension {
    projection: Projection,
    plan: Plan,
}

#[derive(Debug)]
enum Plan {
    Unchanged,
    Replay,
    TryApply {
        batch: Box<ReadableAppended>,
        wanted: Vec<(EventId, TextPlace)>,
    },
}

impl Plan {
    fn for_nonempty(projection: &Projection, records: impl Into<Appended>) -> Self {
        if !projection.extendable() {
            return Self::Replay;
        }
        let Some(batch) = records.into().into_readable() else {
            return Self::Replay;
        };
        let mut wanted = projection.state.wanted_texts(batch.about(projection));
        wanted.sort_unstable_by_key(|(uid, place)| (place.at, *uid));
        Self::TryApply {
            batch: Box::new(batch),
            wanted,
        }
    }
}

impl Projection {
    /// Prepares these records alone for application to this projection. Missing texts or an
    /// unsafe batch make [`Extension::apply`] request replay; preparation never invents data.
    pub fn prepare_extension(self, records: Vec<Record>) -> Extension {
        self.prepare((!records.is_empty()).then(|| OwnedRecords::from(records)))
    }

    /// Prepares an already-read journal batch for the same text loading and application.
    pub fn prepare_appended(self, records: Appended) -> Extension {
        self.prepare((!records.is_empty()).then_some(records))
    }

    fn prepare(self, nonempty: Option<impl Into<Appended>>) -> Extension {
        let plan = nonempty.map_or(Plan::Unchanged, |records| {
            Plan::for_nonempty(&self, records)
        });
        Extension {
            projection: self,
            plan,
        }
    }
}

impl Extension {
    /// The text-file generation these locations belong to, if they came from a store.
    pub fn texts_file(&self) -> Option<[u8; 16]> {
        self.projection.state.texts_file()
    }

    /// The texts this batch needs, each issue once, in location order. An empty batch
    /// or a batch already known to require replay requests none.
    pub fn wanted_texts(&self) -> &[(EventId, TextPlace)] {
        match &self.plan {
            Plan::Unchanged | Plan::Replay => &[],
            Plan::TryApply { wanted, .. } => wanted,
        }
    }

    /// Loads one requested issue from its exact location. A different issue or location is
    /// refused, as are damaged bytes. Successful earlier loads remain usable after a failure.
    pub fn read_texts(
        &mut self,
        uid: EventId,
        place: TextPlace,
        bytes: &[u8],
    ) -> Result<(), Unreadable> {
        let wanted = self.wanted_texts();
        let requested = wanted
            .binary_search_by_key(&(place.at, uid), |(uid, place)| (place.at, *uid))
            .ok()
            .and_then(|at| wanted.get(at));
        if requested != Some(&(uid, place)) {
            return Err(Unreadable("texts outside the prepared extension"));
        }
        self.projection.state.read_texts(uid, place, bytes)
    }

    /// Applies the prepared batch after its texts are loaded, or requests full replay. An
    /// incomplete load cannot turn an unavailable text into an empty one during application.
    pub fn apply(self) -> Result<Extended, KeySpace> {
        let (batch, wanted) = match self.plan {
            Plan::Unchanged => return Ok(Extended::Applied(Box::new(self.projection))),
            Plan::Replay => return Ok(Extended::Replay),
            Plan::TryApply { batch, wanted } => (batch, wanted),
        };
        if wanted.iter().any(|(uid, _)| {
            self.projection
                .state
                .issue(*uid)
                .is_none_or(|issue| !issue.texts().here())
        }) {
            return Ok(Extended::Replay);
        }
        self.projection.extend_readable(*batch)
    }
}
