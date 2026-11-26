//! Public projection preparation, loading, writeback and retry against full replay.

use beadroll_core::id::EventId;
use beadroll_core::json::{Format, Value, write};
use beadroll_core::projection::{
    Appended, Encoded, Extended, OwnedRecords, Projection, Record, project,
};
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

/// A lazy producer yields the first unreadable record and keeps its remaining suffix (AU27).
#[test]
fn collecting_a_batch_stops_the_producer_at_the_first_unreadable_record() {
    let valid = history();
    for (total, bad) in [
        (1024, Some(0)),
        (1024, Some(1)),
        (3, Some(2)),
        (3, None),
        (0, None),
    ] {
        let mut produced = 0;
        let mut source = (0..total).map(|i| {
            produced += 1;
            record(if Some(i) == bad {
                Value::Null
            } else {
                valid[i % valid.len()].clone()
            })
        });
        let batch = Appended::read_records(&mut source);
        let remaining = source.len();
        let expected = bad.map_or(total, |i| i + 1);
        assert_eq!(produced, expected, "first unreadable at {bad:?}");
        assert_eq!(remaining, total - expected);
        assert_eq!(batch.is_readable(), bad.is_none());
    }
}

/// Both forms of an attempt must reject the same record before pulling the next one
/// or requesting texts for a readable prefix (AU28).
#[test]
fn attempt_admission_agrees_for_file_bytes_and_in_memory_records() {
    let mut original = project(journal(&history())).unwrap();
    let saved = original.prepare_rewrite(0).unwrap();
    let projection = decoded(saved.encoded().clone(), saved.generation());
    let wanted = projection.state().wanted_texts([uid(1)]);
    assert_eq!(wanted.len(), 1);
    let set = Event::new(&id(4), "set").deps(&[&id(3)]).issue(&id(1));
    let valid = set
        .clone()
        .data(json!({"fields": {"title": "second"}}))
        .json();
    let suffix = Event::new(&id(5), "set")
        .deps(&[&id(4)])
        .issue(&id(1))
        .data(json!({"fields": {"title": "last"}}))
        .json();
    for (name, middle, readable) in [
        ("valid", valid, true),
        ("non-object", Value::Null, false),
        (
            "bad envelope",
            set.clone().actor(json!({"thread": []})).json(),
            false,
        ),
        ("bad payload", set.data(json!({"fields": []})).json(), false),
    ] {
        let values = [changed(), middle, suffix.clone()];
        let mut file = Vec::new();
        for value in &values {
            file.extend(write(value, Format::COMPACT).unwrap().bytes());
            file.push(b'\n');
        }
        let mut bytes = Appended::new();
        bytes.add_attempt(&file, "a", "test");
        let mut problems = Vec::new();
        let as_values = beadroll_core::journal::read_attempt(&file, "test", Some(&mut problems));
        if readable {
            assert_eq!(as_values, values, "{name}");
            assert!(problems.is_empty());
        } else {
            assert!(as_values.is_empty(), "{name}");
            assert_eq!(problems.len(), 1);
            assert_eq!(problems[0].origin, "test:2");
        }
        let records = values.map(|value| Record {
            sequenced: false,
            ..record(value)
        });
        let mut source = records.iter().cloned();
        let collected = Appended::read_records(&mut source);
        assert_eq!(collected.is_readable(), readable, "{name}");
        assert_eq!(bytes.is_readable(), readable, "{name}");
        assert_eq!(source.len(), usize::from(!readable), "{name}");
        let expected = if readable { wanted.as_slice() } else { &[] };
        for mut prepared in [
            projection.clone().prepare_appended(bytes),
            projection.clone().prepare_appended(collected),
            projection.clone().prepare_extension(records.into()),
        ] {
            assert_eq!(prepared.wanted_texts(), expected, "{name}");
            for &(issue, place) in expected {
                let at = place.at as usize;
                prepared
                    .read_texts(
                        issue,
                        place,
                        &saved.encoded().texts[at..at + place.len as usize],
                    )
                    .unwrap();
            }
            assert_eq!(
                matches!(prepared.apply().unwrap(), Extended::Applied(_)),
                readable,
                "{name}"
            );
        }
    }
}

/// Segments retain malformed records with readable ids for settlement. A future format
/// with a valid payload also passes attempt admission and is judged during replay.
#[test]
fn admission_preserves_segment_and_format_rules_for_both_sources() {
    let set = Event::new(&id(3), "set").deps(&[&id(2)]).issue(&id(1));
    for (value, attempt_ok, segment_ok) in [
        (set.clone().without("id").json(), false, false),
        (set.clone().with("deps", json!([0])).json(), false, true),
        (
            set.clone().data(json!({"fields": false})).json(),
            false,
            true,
        ),
        (
            set.clone()
                .with("v", json!(2))
                .data(json!({"fields": {}}))
                .json(),
            true,
            true,
        ),
        (
            set.with("v", json!(2))
                .data(json!({"fields": false}))
                .json(),
            false,
            true,
        ),
    ] {
        let mut file = write(&value, Format::COMPACT).unwrap().into_bytes();
        file.push(b'\n');
        for (sequenced, readable) in [(true, segment_ok), (false, attempt_ok)] {
            let mut bytes = Appended::new();
            if sequenced {
                bytes.add_segment(&file, "a", "test");
            } else {
                bytes.add_attempt(&file, "a", "test");
            }
            let mut built = Appended::new();
            built.add(Record {
                sequenced,
                ..record(value.clone())
            });
            assert_eq!(
                bytes.is_readable(),
                readable,
                "{value:?}, sequenced={sequenced}"
            );
            assert_eq!(
                built.is_readable(),
                readable,
                "{value:?}, sequenced={sequenced}"
            );
        }
    }
}

/// Values outside the wire representation require replay. The reader keeps signed and
/// unsigned 64-bit integers exact and allows MAX_DEPTH levels including the event object.
#[test]
fn a_caller_built_record_must_round_trip_through_journal_json() {
    use beadroll_core::json::{Int, MAX_DEPTH};
    let nested = |depth| (0..depth).fold(Value::Null, |child, _| Value::Array(vec![child]));
    for (extra, readable) in [
        (Value::Int(Int::from(i64::MIN)), true),
        (Value::Int(Int::from_u64(u64::MAX)), true),
        (Value::Int(Int::from_i128(i128::from(u64::MAX) + 1)), false),
        (Value::Int(Int::from_i128(i128::from(i64::MIN) - 1)), false),
        (Value::Float(f64::INFINITY), false),
        (nested(MAX_DEPTH - 1), true),
        (nested(MAX_DEPTH), false),
    ] {
        let event = Event::new(&id(3), "set")
            .data(json!({"fields": {}}))
            .with("extra", extra)
            .json();
        // Check the fixture against the actual writer and reader.
        let round_trip = write(&event, Format::COMPACT)
            .ok()
            .and_then(|line| beadroll_core::json::parse(line.as_bytes()).ok());
        assert_eq!(
            round_trip.is_some_and(|parsed| parsed.value == event),
            readable
        );
        for sequenced in [false, true] {
            let mut batch = Appended::new();
            batch.add(Record {
                sequenced,
                ..record(event.clone())
            });
            assert_eq!(batch.is_readable(), readable);
        }
    }
}

/// Rejecting a caller-built value must also dispose of the input the API owns, even when
/// the batch or the projection had already been rejected (AU29).
#[test]
fn rejected_records_are_safe_to_discard_at_any_depth() {
    let deep = || {
        let leaves = Value::Array(vec![Value::Null; 1024]);
        let value = (0..30_000).fold(leaves, |child, i| {
            if i % 2 == 0 {
                Value::Array(vec![child])
            } else {
                Value::from(beadroll_core::json::Object::from_iter([(
                    "x".into(),
                    child,
                )]))
            }
        });
        record(value)
    };
    let mut batch = Appended::new();
    batch.add(deep());
    assert!(!batch.is_readable());
    batch.add(deep());
    let batch: Appended = vec![record(Value::Null), deep()].into();
    assert!(!batch.is_readable());

    let mut records = OwnedRecords::from(vec![record(Value::Null), deep(), record(changed())]);
    assert!(!records.read_batch().is_readable());
    assert_eq!(records.remaining().len(), 2);
    assert_eq!(records.remaining()[1].value, changed());
    drop(records);

    let mut records = OwnedRecords::from(vec![deep(), record(changed())]);
    assert!(!records.read_batch().is_readable());
    assert!(records.read_batch().is_readable());
    assert!(records.remaining().is_empty());

    let mut events = history();
    events.push(
        Event::new(&id(9), "set")
            .deps(&[&id(8)])
            .issue(&id(1))
            .data(json!({"fields": {"title": "waiting"}}))
            .json(),
    );
    let waiting = project(journal(&events)).unwrap();
    assert!(!waiting.extendable());
    assert!(matches!(
        waiting.prepare_extension(vec![deep()]).apply().unwrap(),
        Extended::Replay
    ));
}

/// A complete update needs the issue's saved text. A trailing partial line or an in-memory
/// record without an id commits the batch to replay before any text is requested (AU24).
#[test]
fn an_unreadable_batch_requests_no_texts_for_its_valid_prefix() {
    let mut original = project(journal(&history())).unwrap();
    let saved = original.prepare_rewrite(0).unwrap();
    let read = decoded(saved.encoded().clone(), saved.generation());
    let expected = read.state().wanted_texts([uid(1)]);
    assert_eq!(expected.len(), 1);

    let mut line = write(&changed(), Format::COMPACT).unwrap().into_bytes();
    line.push(b'\n');
    let mut batch = Appended::new();
    batch.add_segment(&line, "a", "test");
    let mut complete = read.clone().prepare_appended(batch.clone());
    assert_eq!(complete.wanted_texts(), expected);
    let (issue, place) = expected[0];
    let at = place.at as usize;
    let texts = &saved.encoded().texts[at..at + place.len as usize];
    complete.read_texts(issue, place, texts).unwrap();
    assert!(matches!(complete.apply().unwrap(), Extended::Applied(_)));

    batch.add_segment(b"{", "a", "test");
    for mut unreadable in [
        read.clone().prepare_appended(batch),
        read.prepare_extension(vec![record(changed()), record(Value::Null)]),
    ] {
        assert!(unreadable.wanted_texts().is_empty());
        assert!(unreadable.read_texts(issue, place, texts).is_err());
        assert!(matches!(unreadable.apply().unwrap(), Extended::Replay));
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
    let mut extension = read.prepare_extension(std::iter::once(record(changed())).collect());
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
