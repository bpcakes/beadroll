//! The store acknowledges prepared locations only after both cache writes succeed.

use beadroll_core::projection::Projection;

use super::file::{self, Made};
use super::texts::{self, TextFileAccess};
use super::{SLACK, projection_path, texts_path};
use crate::fs::{abandoned, write_cache_parts};
use crate::workspace::Workspace;

/// Writes `projection` back as made from `made`, when the store is there; a cache that cannot be
/// written is no failure. Its texts written nowhere yet go to the end of the text file its others
/// are placed in, under that file's lock, and the projection after them; when there is no such
/// file, or more of it is no longer read than is, every text goes to a new one, when the
/// projection has them all. What a bead killed writing either file left beside it is removed
/// first, whichever of the two this write-back writes whole (roll-lqjj.52).
pub(super) fn save(ws: &Workspace, mut made: Made, projection: &mut Projection) {
    if made.files.is_empty() || !ws.events_dir.is_dir() {
        return;
    }
    // both caches' at once: the text file is appended to in place, and started again only now
    // and then, so a copy left beside it would otherwise wait for that
    abandoned::remove(&[&projection_path(ws), &texts_path(ws)]);
    ws.settings.failpoints.hit("projection.before-save");
    let generation = projection.state().texts_file();
    if let Some(generation) = generation
        && let Some(file) = texts::open(ws, generation, TextFileAccess::Append)
        && file.lock().is_ok()
        && let Ok(len) = file.metadata().map(|meta| meta.len())
    {
        let read = projection.state().placed_text_bytes();
        // more of the file no longer read than read: every text read in, to write them all again
        let anew = len > texts::START + 2 * read + SLACK && {
            let wanted = projection.state().all_wanted_texts();
            texts::read_in(&file, projection.state_mut(), &wanted)
                && projection.state_mut().write_texts_anew()
        };
        if !anew {
            let Some(write) = projection.prepare_append(len) else {
                return;
            };
            let encoded = write.encoded();
            made.texts = write.generation();
            if texts::append(ws, &file, len, &encoded.texts)
                && write_cache_parts(
                    &projection_path(ws),
                    &file::parts(&file::head(&made, encoded), encoded),
                    &ws.settings.failpoints,
                )
                .is_ok()
            {
                write.written();
            }
            return;
        }
    }
    let Ok(write) = projection.prepare_rewrite(texts::START) else {
        return; // texts placed in a text file no longer there, and not read in
    };
    let encoded = write.encoded();
    let generation = write.generation();
    made.texts = generation;
    if texts::start(ws, generation, &encoded.texts)
        && write_cache_parts(
            &projection_path(ws),
            &file::parts(&file::head(&made, encoded), encoded),
            &ws.settings.failpoints,
        )
        .is_ok()
    {
        write.written();
    }
}
