//! `<store>/projection-texts`: the texts of a projection's issues (roll-lqjj.6). After a header (a
//! magic and the file's generation, the sum of the texts it was started with, which the projection
//! placing texts in it names) come each issue's texts, as bytes the projection's state places and
//! sums. Texts are only ever appended to it, under a lock of the file's own, so that what any
//! projection placed there stays as it was; a projection made whole starts the file again, and one
//! naming another generation than the file's has its texts nowhere. Every read checks the sum of
//! what it reads as well: a file started again with the same texts is the same file.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt as _;
use std::path::PathBuf;

use beadroll_core::id::EventId;
use beadroll_core::state::{State, TextPlace};

use crate::fs::{fsync, write_atomic_parts};
use crate::workspace::Workspace;

/// The first bytes of a text file.
const MAGIC: &[u8; 16] = b"bead texts 1\n\0\0\0";

/// Where its first texts are: after the magic and the generation.
pub(super) const START: u64 = 32;

/// Where a store keeps its projection's texts.
pub fn texts_path(ws: &Workspace) -> PathBuf {
    ws.dir.join("projection-texts")
}

/// The generation of a text file started with `texts`: their sum, so that the same texts make the
/// same file.
pub(super) fn generation(texts: &[u8]) -> [u8; 16] {
    beadroll_core::projection::sum(texts)
}

/// The text file of `generation`, open to read, and to append to when `append`: `None` when there
/// is none, or it is another's.
pub(super) fn open(ws: &Workspace, generation: [u8; 16], append: bool) -> Option<File> {
    let file = (OpenOptions::new().read(true).write(append))
        .open(texts_path(ws))
        .ok()?;
    let mut head = [0u8; START as usize];
    file.read_exact_at(&mut head, 0).ok()?;
    (head.get(..16) == Some(&MAGIC[..]) && head.get(16..) == Some(&generation[..])).then_some(file)
}

/// Reads into `state` the texts at `wanted` from `file`: `false` as soon as one is not as written.
pub(super) fn read_in(file: &File, state: &mut State, wanted: &[(EventId, TextPlace)]) -> bool {
    let mut bytes = Vec::new();
    for (uid, place) in wanted {
        bytes.resize(place.len as usize, 0);
        if file.read_exact_at(&mut bytes, place.at).is_err()
            || state.read_texts(*uid, *place, &bytes).is_err()
        {
            return false;
        }
    }
    true
}

/// Starts the text file again, of `generation`, holding `texts` after its header: whole and durable,
/// in place of the one there, or not at all.
pub(super) fn start(ws: &Workspace, generation: [u8; 16], texts: &[u8]) -> bool {
    // the texts written as they are, not first copied behind the header (roll-lqjj.9)
    let parts: [&[u8]; 3] = [MAGIC, &generation, texts];
    write_atomic_parts(&texts_path(ws), &parts, true, &ws.settings.failpoints).is_ok()
}

/// Appends `texts` to `file`, `at` its length, durably.
pub(super) fn append(ws: &Workspace, file: &File, at: u64, texts: &[u8]) -> bool {
    if texts.is_empty() {
        return true;
    }
    let written = file.write_all_at(texts, at).is_ok() && fsync(file).is_ok();
    ws.settings.failpoints.hit("projection.texts-appended");
    written
}
