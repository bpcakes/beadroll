//! Public projection preparation, loading, writeback and retry against full replay.

use beadroll_core::id::EventId;
use beadroll_core::json::Value;
use beadroll_core::projection::{Encoded, Extended, Projection, Record, project};
use beadroll_core::replay::replay;
use beadroll_core::state::{State, Texts};
use beadroll_test_support::events::{Event, id, journal};
use beadroll_test_support::json;

fn uid(n: u64) -> EventId {
    EventId::parse(&id(n)).unwrap()
}

fn history() -> Vec<Value> {
    vec![
        Event::new(&id(0), "init")
            .data(json!({"prefix": "ex"}))
            .json(),
        Event::new(&id(1), "create")
            .deps(&[&id(0)])
            .data(json!({"key": "ex-one", "fields": {"description": "before"}}))
            .json(),
        Event::new(&id(2), "create")
            .deps(&[&id(1)])
            .data(json!({"key": "ex-two", "fields": {"notes": "unrelated"}}))
            .json(),
    ]
}

fn changed() -> Value {
    Event::new(&id(3), "set")
        .deps(&[&id(2)])
        .issue(&id(1))
        .data(json!({"fields": {"description": "after"}}))
        .json()
}

fn record(value: Value) -> Record {
    Record {
        value,
        replica: "a".into(),
        origin: "test".into(),
        sequenced: true,
    }
}

fn decoded(encoded: Encoded, generation: [u8; 16]) -> Projection {
    let mut projection = Projection::decode(encoded).unwrap();
    projection.state_mut().set_texts_file(Some(generation));
    projection
}

fn load(state: &mut State, file: &[u8]) {
    for (uid, place) in state.all_wanted_texts() {
        let at = place.at as usize;
        state
            .read_texts(uid, place, &file[at..at + place.len as usize])
            .unwrap();
    }
}

#[test]
fn preparation_loads_only_its_batch_and_a_written_extension_reloads_as_replay() {
    let mut events = history();
    let mut original = project(journal(&events)).unwrap();
    let write = original.prepare_rewrite(0).unwrap();
    let (encoded, generation) = (write.encoded().clone(), write.generation());
    let mut file = encoded.texts.clone();
    write.written();

    let read = decoded(encoded, generation);
    let unrequested = read.state().all_wanted_texts()[1];
    assert!(matches!(
        read.clone()
            .prepare_extension(vec![record(changed())])
            .apply()
            .unwrap(),
        Extended::Replay
    ));
    let mut extension = read.prepare_extension(vec![record(changed())]);
    let [(issue, place)] = extension.wanted_texts() else {
        panic!("one issue requested")
    };
    let (issue, place) = (*issue, *place);
    assert_eq!(issue, uid(1));
    assert!(
        extension
            .read_texts(unrequested.0, unrequested.1, &[])
            .is_err()
    );
    assert!(extension.read_texts(issue, place, b"corrupt").is_err());
    let at = place.at as usize;
    extension
        .read_texts(issue, place, &file[at..at + place.len as usize])
        .unwrap();
    let Extended::Applied(mut next) = extension.apply().unwrap() else {
        panic!("loaded extension")
    };
    assert!(next.state().issue(uid(1)).unwrap().hydrated().is_ok());
    assert!(next.state().issue(uid(2)).unwrap().hydrated().is_err());

    // An append writes only the changed issue and keeps the other issue's old location.
    let write = next.prepare_append(file.len() as u64).unwrap();
    let after = write.encoded().clone();
    assert_eq!(write.generation(), generation);
    assert!(!after.texts.is_empty());
    file.extend_from_slice(&after.texts);
    write.written();
    let mut reloaded = decoded(after, generation);
    load(reloaded.state_mut(), &file);
    events.push(changed());
    let full = replay(journal(&events)).unwrap();
    assert_eq!(
        reloaded.state().hydrated().unwrap().export_records(),
        full.hydrated().unwrap().export_records()
    );
}

#[test]
fn abandoned_writebacks_do_not_acknowledge_locations_and_can_be_retried() {
    let mut projection = project(journal(&history())).unwrap();
    let write = projection.prepare_rewrite(0).unwrap();
    let first = write.encoded().clone();
    let generation = write.generation();
    drop(write);
    assert!(
        projection
            .state()
            .issues()
            .all(|issue| matches!(issue.texts(), Texts::Here))
    );
    assert_eq!(projection.state().texts_file(), None);
    let retry = projection.prepare_rewrite(0).unwrap();
    assert_eq!(retry.encoded().state, first.state);
    assert_eq!(retry.encoded().texts, first.texts);
    assert_eq!(retry.generation(), generation);
    retry.written();

    let Extended::Applied(mut changed) = projection
        .prepare_extension(vec![record(changed())])
        .apply()
        .unwrap()
    else {
        panic!("in-memory extension")
    };
    let write = changed.prepare_append(first.texts.len() as u64).unwrap();
    let pending = write.encoded().texts.clone();
    assert!(!pending.is_empty());
    drop(write);
    assert!(matches!(
        changed.state().issue(uid(1)).unwrap().texts(),
        Texts::Here
    ));
    let retry = changed.prepare_append(first.texts.len() as u64).unwrap();
    assert_eq!(retry.encoded().texts, pending);
    retry.written();
    assert!(matches!(
        changed.state().issue(uid(1)).unwrap().texts(),
        Texts::Read(_)
    ));
}

#[test]
fn rewriting_requires_all_texts_and_preserves_unloaded_locations_on_refusal() {
    let mut original = project(journal(&history())).unwrap();
    let write = original.prepare_rewrite(0).unwrap();
    let (encoded, generation) = (write.encoded().clone(), write.generation());
    let mut read = decoded(encoded.clone(), generation);
    let (issue, place) = read.state().all_wanted_texts()[0];
    let at = place.at as usize;
    read.state_mut()
        .read_texts(issue, place, &encoded.texts[at..at + place.len as usize])
        .unwrap();
    let before = read.state().all_wanted_texts();
    assert_eq!(read.prepare_rewrite(0).unwrap_err().issue, uid(2));
    assert_eq!(read.state().all_wanted_texts(), before);
    assert!(matches!(
        read.state().issue(uid(1)).unwrap().texts(),
        Texts::Read(_)
    ));
    load(read.state_mut(), &encoded.texts);
    let rewrite = read.prepare_rewrite(0).unwrap();
    assert_eq!(rewrite.encoded().state, encoded.state);
    assert_eq!(rewrite.encoded().texts, encoded.texts);
}
