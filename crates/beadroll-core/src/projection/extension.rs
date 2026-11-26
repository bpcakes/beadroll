//! Preparation for applying an exact batch to a projection or requesting replay.

use crate::id::EventId;
use crate::state::{KeySpace, TextPlace};

use super::{Appended, Extended, ExtensionBatch, Projection, Unreadable};

/// An owned projection with the exact batch and text locations it can try to apply.
/// A projection that already requires replay discards a nonempty batch before conversion.
/// Obtain this from [`Projection::prepare_extension`], load [`Self::wanted_texts`], then
/// consume it with [`Self::apply`].
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
    // None when the projection requires full replay before it can take any more records.
    records: Option<Appended>,
    wanted: Vec<(EventId, TextPlace)>,
}

impl Projection {
    /// Prepares these records alone for application to this projection. Missing texts or an
    /// unsafe batch make [`Extension::apply`] request replay; preparation never invents data.
    pub fn prepare_extension(self, records: impl ExtensionBatch) -> Extension {
        let records = (self.extendable() || records.is_empty()).then(|| records.into());
        let mut wanted = records
            .as_ref()
            .map_or_else(Vec::new, |batch| self.state.wanted_texts(self.about(batch)));
        wanted.sort_unstable_by_key(|(uid, place)| (place.at, *uid));
        Extension {
            projection: self,
            records,
            wanted,
        }
    }
}

impl Extension {
    /// The text-file generation these locations belong to, if they came from a store.
    pub fn texts_file(&self) -> Option<[u8; 16]> {
        self.projection.state.texts_file()
    }

    /// The texts this batch needs, each issue once, in location order.
    pub fn wanted_texts(&self) -> &[(EventId, TextPlace)] {
        &self.wanted
    }

    /// Loads one requested issue from its exact location. A different issue or location is
    /// refused, as are damaged bytes. Successful earlier loads remain usable after a failure.
    pub fn read_texts(
        &mut self,
        uid: EventId,
        place: TextPlace,
        bytes: &[u8],
    ) -> Result<(), Unreadable> {
        let requested = self
            .wanted
            .binary_search_by_key(&(place.at, uid), |(uid, place)| (place.at, *uid))
            .ok()
            .and_then(|at| self.wanted.get(at));
        if requested != Some(&(uid, place)) {
            return Err(Unreadable("texts outside the prepared extension"));
        }
        self.projection.state.read_texts(uid, place, bytes)
    }

    /// Applies the prepared batch after its texts are loaded, or requests full replay. An
    /// incomplete load cannot turn an unavailable text into an empty one during application.
    pub fn apply(self) -> Result<Extended, KeySpace> {
        let Some(records) = self.records else {
            return Ok(Extended::Replay);
        };
        if self.wanted.iter().any(|(uid, _)| {
            self.projection
                .state
                .issue(*uid)
                .is_none_or(|issue| !issue.texts().here())
        }) {
            return Ok(Extended::Replay);
        }
        self.projection.extend(records)
    }
}
