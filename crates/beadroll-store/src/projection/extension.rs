//! Load the exact texts selected by an owned core extension before applying it.

use beadroll_core::projection::{Appended, Extended, Extension, Projection, Record};
use beadroll_core::state::KeySpace;

use super::texts::{self, TextFileAccess};
use crate::workspace::Workspace;

/// Applies a batch to its projection after loading only that batch's texts. Missing, replaced
/// or damaged text files request replay, as does an extension the core cannot safely apply.
pub fn extend_projection(
    ws: &Workspace,
    projection: Projection,
    records: Vec<Record>,
) -> Result<Extended, KeySpace> {
    apply(ws, projection.prepare_extension(records))
}

/// Applies a journal batch already read from appended lines or in-memory records.
pub fn extend_appended(
    ws: &Workspace,
    projection: Projection,
    records: Appended,
) -> Result<Extended, KeySpace> {
    apply(ws, projection.prepare_appended(records))
}

fn apply(ws: &Workspace, mut extension: Extension) -> Result<Extended, KeySpace> {
    if !extension.wanted_texts().is_empty() {
        let file = extension
            .texts_file()
            .and_then(|generation| texts::open(ws, generation, TextFileAccess::Read));
        let Some(file) = file else {
            return Ok(Extended::Replay);
        };
        let wanted = extension.wanted_texts().to_vec();
        if !texts::read_with(&file, &wanted, |uid, place, bytes| {
            extension.read_texts(uid, place, bytes).is_ok()
        }) {
            return Ok(Extended::Replay);
        }
    }
    extension.apply()
}
