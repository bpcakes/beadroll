//! Prepared cache bytes and their acknowledgement belong to one borrowed projection.

use crate::id::EventId;
use crate::state::{MissingTexts, TextPlace};

use super::{Encoded, Projection, sum};

/// Bytes prepared for one projection and one text-file generation. The exclusive borrow keeps
/// the state unchanged until the writer either acknowledges success with [`Self::written`] or
/// drops the preparation. Dropping it does not acknowledge any text locations.
///
/// The core performs no I/O: the writer must persist the texts and projection before calling
/// `written`. A failed or interrupted cache write may always be discarded and replayed.
///
/// ```compile_fail,E0499
/// use beadroll_core::projection::Projection;
/// fn change_while_writing(projection: &mut Projection) {
///     let write = projection.prepare_rewrite(0).unwrap();
///     projection.state_mut().write_texts_anew();
///     write.written();
/// }
/// ```
///
/// ```compile_fail,E0624
/// use beadroll_core::projection::Projection;
/// fn acknowledge_other(source: &Projection, other: &mut Projection) {
///     let (_, places) = source.encode(0);
///     other.placed(&places);
/// }
/// ```
#[derive(Debug)]
#[must_use = "persist both parts before acknowledging, or drop without acknowledging"]
pub struct Writeback<'a> {
    projection: &'a mut Projection,
    encoded: Encoded,
    places: Vec<(EventId, TextPlace)>,
    generation: [u8; 16],
}

impl Projection {
    /// Prepares new texts for appending at `texts_at` in this projection's text-file generation.
    /// The writer must have opened and locked that generation and measured its current length.
    /// A projection without a generation needs [`Self::prepare_rewrite`] instead.
    pub fn prepare_append(&mut self, texts_at: u64) -> Option<Writeback<'_>> {
        let generation = self.state.texts_file()?;
        Some(self.prepare_writeback(texts_at, generation))
    }

    /// Prepares every text for a new file, starting at `texts_at` after the writer's header.
    /// All texts must be loaded. The new generation is derived from these exact bytes. If the
    /// writer fails, the texts stay loaded and marked for writing again.
    pub fn prepare_rewrite(&mut self, texts_at: u64) -> Result<Writeback<'_>, MissingTexts> {
        self.state.hydrated()?;
        self.state.write_texts_anew();
        let (encoded, places) = self.encode(texts_at);
        let generation = sum(&encoded.texts);
        Ok(Writeback {
            projection: self,
            encoded,
            places,
            generation,
        })
    }

    fn prepare_writeback(&mut self, texts_at: u64, generation: [u8; 16]) -> Writeback<'_> {
        let (encoded, places) = self.encode(texts_at);
        Writeback {
            projection: self,
            encoded,
            places,
            generation,
        }
    }
}

impl Writeback<'_> {
    /// The immutable state, index and text bytes to persist together.
    pub fn encoded(&self) -> &Encoded {
        &self.encoded
    }

    /// The generation the projection's text locations refer to.
    pub fn generation(&self) -> [u8; 16] {
        self.generation
    }

    /// Acknowledges that both the texts and projection were successfully written. Only this
    /// preparation's projection receives its locations and generation; neither can be supplied
    /// by the caller or used to acknowledge another version.
    pub fn written(self) {
        self.projection.placed(&self.places);
        self.projection.state.set_texts_file(Some(self.generation));
    }
}
