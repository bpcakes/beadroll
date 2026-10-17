//! What a write leaves of the store's projection (roll-lqjj.51): the projection its transaction
//! holds, with every event it emitted, written back once the store lock is let go of, as a read
//! writes back what it applied, so that the read after the write finds it current. It is made from
//! the journal's files as the write left them, which the write names under the lock it still
//! holds: the files the projection was read from, as it read them, but for its own segment, grown
//! by the events it appended or begun with them, and the attempt file of the ownership events it
//! confirmed here. Anything else in the journal changed, and nothing is written back: the next
//! command extends the stored projection or replays the journal, as it would have.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use beadroll_core::id::EventId;
use beadroll_core::journal::{Problem, read_segment};
use beadroll_core::json::Value;
use beadroll_core::projection::Projection;

use crate::failpoint::Fired;
use crate::fs::{content_sum, read_keyed};
use crate::journal::{attempt_events, attempt_path, segment_files};
use crate::workspace::Workspace;

use super::file::{Held, Made};
use super::{attempts, held, save};

/// What a projection was made from, as one written back names it: the build that read it and each
/// journal file's identity and sum, as they were read.
#[derive(Clone, Debug)]
pub struct Basis {
    pub(super) made: Made,
}

/// The journal's files as a write that went on from a projection made from `basis` left them,
/// under the store lock it still holds: one segment grown by the events `appended`, in their order,
/// or begun with them, and the attempt file of the ownership events `confirmed`, when there are
/// any, added; every other file as the basis has it. `None` when the journal is not just that,
/// and nothing is then written back.
pub fn after_write(
    ws: &Workspace,
    basis: &Basis,
    appended: &[EventId],
    confirmed: &[Value],
) -> Option<Basis> {
    let before: HashMap<&str, &Held> = (basis.made.files.iter())
        .map(|held| (held.path.as_str(), held))
        .collect();
    let mut files: Vec<Held> = Vec::with_capacity(basis.made.files.len() + 2);
    let mut grown = appended.is_empty();
    for now in held(ws, &segment_files(ws).ok()?, true)? {
        match before.get(now.path.as_str()) {
            Some(was) if was.segment && was.key == now.key => files.push((*was).clone()),
            // the write's own, which nothing else writes while it holds the lock
            was if !grown => {
                files.push(own_segment(ws, was.copied(), &now, appended)?);
                grown = true;
            }
            _ => return None,
        }
    }
    let own = (!confirmed.is_empty()).then(|| attempt_path(ws, confirmed));
    let mut added = own.is_none();
    let listed = attempts(ws).ok()??;
    for (path, now) in listed.paths.iter().zip(listed.held) {
        match before.get(now.path.as_str()) {
            Some(was) if !was.segment && was.key == now.key => files.push((*was).clone()),
            None if !added && own.as_deref() == Some(path.as_path()) => {
                files.push(own_attempt(path, now, confirmed)?);
                added = true;
            }
            _ => return None,
        }
    }
    let kept: HashSet<&str> = files.iter().map(|held| held.path.as_str()).collect();
    let whole = grown && added && before.keys().all(|path| kept.contains(path));
    whole.then(|| Basis {
        made: Made {
            build: basis.made.build.clone(),
            files,
            texts: basis.made.texts,
        },
    })
}

/// The segment `now`, which the write appended `appended` to: grown from what the basis kept of it
/// (`was`), the bytes before the ones summed, or begun with them; what a projection keeps of it,
/// its sum the whole file's. `None` when the bytes it holds are not those.
fn own_segment(
    ws: &Workspace,
    was: Option<&Held>,
    now: &Held,
    appended: &[EventId],
) -> Option<Held> {
    let path = ws.events_dir.join(&now.path);
    let (data, key) = read_keyed(&path).ok()?;
    let key = key.filter(|key| *key == now.key)?;
    let from = match was {
        Some(was) if key.same_file(&was.key) && key.len() > was.key.len() => {
            usize::try_from(was.key.len()).ok()?
        }
        Some(_) => return None,
        None => 0,
    };
    let (before, after) = (data.get(..from)?, data.get(from..)?);
    if was.is_some_and(|was| was.sum != Some(content_sum(before))) {
        return None;
    }
    let mut problems: Vec<Problem> = Vec::new();
    let read = read_segment(after, &path.display().to_string(), Some(&mut problems));
    let whole = after.last() == Some(&b'\n') && problems.is_empty();
    (whole && ids(&read)? == appended).then(|| Held {
        path: now.path.clone(),
        key,
        segment: true,
        sum: Some(content_sum(&data)),
    })
}

/// The attempt file at `path`, listed as `now`, which the write wrote with the ownership events
/// `confirmed`: what a projection keeps of it, with the sum of its bytes. `None` when it holds
/// other events.
fn own_attempt(path: &Path, now: Held, confirmed: &[Value]) -> Option<Held> {
    let (data, key) = read_keyed(path).ok()?;
    let key = key.filter(|key| *key == now.key)?;
    let mut problems: Vec<Problem> = Vec::new();
    let read = attempt_events(path, Some(&data), Some(&mut problems));
    (problems.is_empty() && ids(&read)? == ids(confirmed)?).then(|| Held {
        key,
        sum: Some(content_sum(&data)),
        ..now
    })
}

/// The ids of `events`, in their order; `None` when one has none.
fn ids(events: &[Value]) -> Option<Vec<EventId>> {
    (events.iter())
        .map(|event| {
            event
                .get("id")
                .and_then(Value::as_str)
                .and_then(EventId::parse)
        })
        .collect()
}

/// Writes `projection` back as made from `basis`, as a read writes back what it applied: a cache,
/// so one that cannot be written leaves the stored projection as it was, and the write it follows
/// has gone through whatever happens here. `write.save` stops a write there, or leaves its
/// projection to the next command (`skip`).
pub fn write_back(ws: &Workspace, basis: Basis, projection: &mut Projection) {
    if ws.settings.failpoints.hit("write.save") == Some(Fired::Skip) {
        return;
    }
    save(ws, basis.made, projection);
}
