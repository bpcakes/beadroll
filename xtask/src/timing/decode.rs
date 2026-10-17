//! `timing --decode N`: what reading a store's projection of N events costs in this process
//! (roll-lqjj.35, X16): its state part decoded alone, as a command that only reads decodes it
//! (`decode_state`), and the whole projection, as a write decodes it with its index
//! (`Projection::decode`), the fastest of [`RUNS`] runs each; the heap a decoded state holds, as
//! xtask's allocator counts it; and how many bytes the state part, the index and the texts take.
//! And what writing it back costs (roll-lqjj.43, X17): the projection read back and extended by a
//! comment, encoded as a write-back between compactions encodes it; the same at the worst point,
//! the projection having taken in as many comments as it can before a write-back compacts it, an
//! eighth of the events it knew ([`COMPACTED`]), none of which its state names: its index's bytes,
//! its decode, the heap it holds whole and its encode, beside the heap the projection holds whole
//! as it was made; and the encode of the write-back one comment later, which compacts it. Replay and the files around a projection
//! are not measured: `timing --read N` times the commands themselves. Evidence, never a gate.

use std::time::Instant;

use beadroll_core::id::EventId;
use beadroll_core::json::{Object, Value};
use beadroll_core::projection::{
    COMPACTED, Encoded, Extended, Projection, Record, decode_state, project,
};

use super::heap;
use super::history::{Shape, event_id, history};
use super::memory;
use super::world::profile;
use crate::workspace::{self, Failure, json_string};

/// How many times each part is decoded; the fastest run counts.
const RUNS: usize = 7;

/// The fastest of [`RUNS`] runs of `run`, in seconds.
fn fastest<T>(mut run: impl FnMut() -> Result<T, Failure>) -> Result<f64, Failure> {
    let mut best = f64::INFINITY;
    for _ in 0..RUNS {
        let clock = Instant::now();
        let made = run()?;
        best = best.min(clock.elapsed().as_secs_f64());
        drop(made);
    }
    Ok(best)
}

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(count: usize, shape: Shape, label: Option<&str>) -> Result<String, Failure> {
    let dir = workspace::output_dir("timing")?.join(format!("decode-{}-{count}", shape.name()));
    memory::write_files(&dir, &history(count, shape))?;
    let journal = memory::load(&dir)?;
    // An ownership-heavy journal is a hundred thousand files: nothing to leave lying about.
    std::fs::remove_dir_all(&dir).map_err(|error| Failure::io("cannot remove", &dir, &error))?;
    let projection =
        project(journal).map_err(|error| Failure::Failed(format!("replay failed: {error}")))?;
    let issues = projection.state().issues().count();
    let (encoded, _) = projection.encode(0);
    let unreadable = |error| Failure::Failed(format!("the projection does not read back: {error}"));
    let state_seconds = fastest(|| decode_state(&encoded.state).map_err(unreadable))?;
    let projection_seconds = fastest(|| Projection::decode(&encoded).map_err(unreadable))?;
    let before = heap::held();
    let state = decode_state(&encoded.state).map_err(unreadable)?;
    let state_heap = heap::held().saturating_sub(before);
    drop(state);
    let projection_heap = held_whole(&encoded)?;
    // a write-back after a comment, between compactions
    let read = Projection::decode(&encoded).map_err(unreadable)?;
    let once = taken(read, &encoded.texts, &comments(&projection, 1))?;
    let encode_seconds = fastest(|| Ok(once.encode(0)))?;
    drop(once);
    // the worst point: as many comments as it takes before a write-back compacts it
    let known = projection.known();
    let read = Projection::decode(&encoded).map_err(unreadable)?;
    let worst = taken(
        read,
        &encoded.texts,
        &comments(&projection, known / COMPACTED),
    )?;
    if worst.compaction_due() {
        return Err(Failure::Failed("the worst point compacts".into()));
    }
    let (worst_encoded, _) = worst.encode(0);
    let worst_known = worst.known();
    let worst_encode_seconds = fastest(|| Ok(worst.encode(0)))?;
    drop(worst);
    // and one comment more: the write-back that compacts it
    let read = Projection::decode(&encoded).map_err(unreadable)?;
    let more = comments(&projection, known / COMPACTED + 1);
    let past = taken(read, &encoded.texts, &more)?;
    drop(projection);
    if !past.compaction_due() {
        return Err(Failure::Failed(
            "one comment past the worst point does not compact".into(),
        ));
    }
    let compacting_seconds = fastest(|| Ok(past.encode(0)))?;
    drop(past);
    let worst_seconds = fastest(|| Projection::decode(&worst_encoded).map_err(unreadable))?;
    let worst_heap = held_whole(&worst_encoded)?;
    let label = label.map_or_else(
        || format!("decode of {count} {}", shape.events()),
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"shape\": {}, \"events\": {count}, \"issues\": {issues}, \
         \"profile\": {}, \"runs\": {RUNS}, \"state_seconds\": {state_seconds:.4}, \
         \"projection_seconds\": {projection_seconds:.4}, \"state_heap_bytes\": {state_heap}, \
         \"state_bytes\": {}, \"index_bytes\": {}, \"texts_bytes\": {}, \
         \"projection_heap_bytes\": {projection_heap}, \"known\": {known}, \
         \"encode_seconds\": {encode_seconds:.4}, \
         \"compacting_encode_seconds\": {compacting_seconds:.4}, \
         \"worst_known\": {worst_known}, \"worst_index_bytes\": {}, \
         \"worst_projection_seconds\": {worst_seconds:.4}, \
         \"worst_projection_heap_bytes\": {worst_heap}, \
         \"worst_encode_seconds\": {worst_encode_seconds:.4}, ",
        json_string(&label),
        json_string(shape.name()),
        json_string(profile()),
        encoded.state.len(),
        encoded.index.len(),
        encoded.texts.len(),
        worst_encoded.index.len(),
    ))
}

/// The heap a projection holds decoded whole from `encoded`, with its index, as a write holds it.
fn held_whole(encoded: &Encoded) -> Result<usize, Failure> {
    let before = heap::held();
    let projection = Projection::decode(encoded)
        .map_err(|error| Failure::Failed(format!("the projection does not read back: {error}")))?;
    let held = heap::held().saturating_sub(before);
    drop(projection);
    Ok(held)
}

/// `projection`, read back from bytes whose texts are `texts`, with `records` applied after it, as a
/// store's next command applies them: the texts of the issues they are about read in first.
fn taken(
    mut projection: Projection,
    texts: &[u8],
    records: &[Record],
) -> Result<Projection, Failure> {
    let about = (records.iter())
        .filter_map(|record| record.value.get("issue").and_then(Value::as_str))
        .filter_map(EventId::parse);
    let state = projection.state_mut();
    for (uid, place) in state.wanted_texts(about) {
        let at = usize::try_from(place.at).unwrap_or(usize::MAX);
        let bytes = (texts.get(at..at.saturating_add(place.len as usize)))
            .ok_or_else(|| Failure::Failed("the texts are not where they were placed".into()))?;
        (state.read_texts(uid, place, bytes))
            .map_err(|error| Failure::Failed(format!("the texts do not read back: {error}")))?;
    }
    match projection.extend(records.to_vec()) {
        Ok(Extended::Applied(extended)) => Ok(*extended),
        Ok(Extended::Replay) => Err(Failure::Failed("the comments are declined".into())),
        Err(error) => Err(Failure::Failed(format!(
            "the comments do not apply: {error}"
        ))),
    }
}

/// `count` comments on the first issue of `projection`, by the first machine with a journal there,
/// each after the one before, the first after its frontier: records appended to that machine's
/// journal after the history's, which no state names once applied.
fn comments(projection: &Projection, count: usize) -> Vec<Record> {
    let Some(&(machine, length, _)) = projection.journals().first() else {
        return Vec::new();
    };
    let issue = (projection.state().issues().next())
        .map_or_else(String::new, |issue| issue.uid().to_string());
    let mut deps: Vec<Value> = (projection.heads().iter())
        .map(|head| Value::from(head.to_string()))
        .collect();
    let held = projection.event_count();
    let mut records = Vec::with_capacity(count);
    for n in 0..count {
        let id = event_id(held + n);
        let mut data = Object::new();
        data.insert("text".into(), Value::from(format!("taken in {n}")));
        let mut event = Object::new();
        event.insert("v".into(), Value::from(1));
        event.insert("id".into(), Value::from(id.as_str()));
        event.insert("replica".into(), Value::from(machine));
        event.insert("deps".into(), Value::Array(std::mem::take(&mut deps)));
        event.insert("op".into(), Value::from("comment"));
        event.insert("issue".into(), Value::from(issue.as_str()));
        event.insert("data".into(), Value::from(data));
        records.push(Record {
            value: Value::from(event),
            replica: machine.to_owned(),
            origin: format!("events/{machine}/{:06}.jsonl", length),
            sequenced: true,
        });
        deps = vec![Value::from(id)];
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worst_point_is_an_eighth_more_events_than_a_compaction_leaves_and_one_more_compacts() {
        let projection = project(history_journal(2000)).unwrap();
        let known = projection.known();
        assert_eq!(known, projection.compacted());
        let (encoded, _) = projection.encode(0);
        let read = Projection::decode(&encoded).unwrap();
        let worst = taken(
            read,
            &encoded.texts,
            &comments(&projection, known / COMPACTED),
        )
        .unwrap();
        assert_eq!(worst.known(), known + known / COMPACTED);
        assert!(!worst.compaction_due(), "written back as it is");
        let read = Projection::decode(&encoded).unwrap();
        let more = comments(&projection, known / COMPACTED + 1);
        let past = taken(read, &encoded.texts, &more).unwrap();
        assert!(past.compaction_due(), "one comment more, and it compacts");
        let (compacted, _) = past.encode(0);
        let compacted = Projection::decode(&compacted).unwrap();
        // the comments, named by nothing, are gone, but for the last, the frontier
        let ids: Vec<EventId> = (more.iter())
            .filter_map(|record| record.value.get("id").and_then(Value::as_str))
            .filter_map(EventId::parse)
            .collect();
        let (last, earlier) = ids.split_last().unwrap();
        assert_eq!(compacted.heads(), [*last]);
        assert!(compacted.state().holds(*last));
        assert!(earlier.iter().all(|id| !compacted.state().holds(*id)));
        assert!(compacted.known() <= known + 1);
    }

    /// A history of `count` events, as `measure` loads it.
    fn history_journal(count: usize) -> beadroll_core::journal::Journal {
        let dir = tempfile::tempdir().unwrap();
        memory::write_files(dir.path(), &history(count, Shape::Plain)).unwrap();
        memory::load(dir.path()).unwrap()
    }
}
