//! A projection and the exact batch whose texts it is preparing to apply.

use crate::id::EventId;
use crate::state::{KeySpace, TextPlace};

use super::{Extended, Projection, Record, Unreadable};

/// An owned projection and record batch, with their selected text locations. Obtain it from
/// [`Projection::prepare_extension`], load [`Self::wanted_texts`], then consume it with
/// [`Self::apply`]. Neither the projection nor the batch can change during preparation.
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
    records: Vec<Record>,
    wanted: Vec<(EventId, TextPlace)>,
}

impl Projection {
    /// Prepares these records alone for application to this projection. Missing texts or an
    /// unsafe batch make [`Extension::apply`] request replay; preparation never invents data.
    pub fn prepare_extension(self, records: Vec<Record>) -> Extension {
        let mut wanted = self.state.wanted_texts(self.about(&records));
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
        if self.wanted.iter().any(|(uid, _)| {
            self.projection
                .state
                .issue(*uid)
                .is_none_or(|issue| !issue.texts().here())
        }) {
            return Ok(Extended::Replay);
        }
        self.projection.extend(self.records)
    }
}
