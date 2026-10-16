//! The segment reader (roll-k2wy.14.4). Any bytes, read as a journal segment and as an attempt
//! file: what cannot be read is reported, never a panic; an attempt read whole holds what the
//! segment reader reads of the same bytes; the values read, written again as journal lines, read
//! back alike with nothing reported. Then what was read goes where a store would find it, each
//! record in the journal of the machine it names, and is replayed, which is total: whichever order
//! the journals are read in, with each journal's own order kept, the state answers alike (what
//! could not be used compared as a set, being reported in the order found); its export is written
//! and read back by the importer's reader; and the projection of the same journal, encoded and
//! decoded with its texts read back, answers as replay does.
#![no_main]

use std::collections::{BTreeMap, BTreeSet};

use beadroll_core::event::event_problem;
use beadroll_core::import::read_records;
use beadroll_core::journal::{Journal, read_attempt, read_segment};
use beadroll_core::json::{self, Format, Value};
use beadroll_core::projection::{Projection, project};
use beadroll_core::replay::replay;
use beadroll_fuzz::{all_identical, answers, first_difference};
use libfuzzer_sys::fuzz_target;

/// The machine whose journal a record that names none is found in.
const HERE: &str = "0000aa01";

/// Whether an id of `records` has several contents and not exactly one of them is its writer's own
/// (a valid event in the journal of the machine it names): the journal then places it by the first
/// own copy it found, or the first copy, so what replay reports of it depends on the order the
/// journals were read in, which a store keeps the same, their names' (roll-k2wy.14.14).
fn placed_by_order(records: &[Value]) -> bool {
    let mut copies: BTreeMap<&str, (BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();
    for record in records {
        let Some(id) = record.get("id").and_then(Value::as_str) else {
            continue;
        };
        let named = record
            .get("replica")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let own = event_problem(record).is_none() && !named.is_empty() && !named.contains('/');
        let content = json::write(record, Format::CANONICAL).unwrap_or_default();
        let (contents, owned) = copies.entry(id).or_default();
        if own {
            owned.insert(content.clone());
        }
        contents.insert(content);
    }
    copies
        .values()
        .any(|(contents, owned)| contents.len() > 1 && owned.len() != 1)
}

fuzz_target!(|data: &[u8]| {
    let mut problems = Vec::new();
    let read = read_segment(data, "events/0000aa01/000001.jsonl", Some(&mut problems));
    let attempt = read_attempt(data, "events/0000aa01/claims/a.json", Some(&mut problems));
    if !attempt.is_empty() {
        assert!(all_identical(&attempt, &read), "an attempt read otherwise");
    }
    let mut lines = String::new();
    for value in &read {
        lines.push_str(&json::write(value, Format::COMPACT).expect("a value read is written"));
        lines.push('\n');
    }
    problems.clear();
    let again = read_segment(
        lines.as_bytes(),
        "events/0000aa01/000002.jsonl",
        Some(&mut problems),
    );
    assert!(
        problems.is_empty(),
        "lines bead wrote are reported: {problems:?}"
    );
    assert!(
        all_identical(&again, &read),
        "lines bead wrote read back otherwise"
    );

    // Each record in the journal of the machine it names, the attempt's in an attempt file.
    let mut machines: Vec<(String, Vec<&Value>)> = Vec::new();
    for value in &read {
        let named = value.get("replica").and_then(Value::as_str);
        let machine = named
            .filter(|m| !m.is_empty() && !m.contains('/'))
            .unwrap_or(HERE);
        match machines.iter_mut().find(|(m, _)| m == machine) {
            Some((_, records)) => records.push(value),
            None => machines.push((machine.to_owned(), vec![value])),
        }
    }
    let journal = |machines: &[(String, Vec<&Value>)]| {
        let mut journal = Journal::new();
        for (machine, records) in machines {
            let origin = format!("events/{machine}/000001.jsonl");
            for record in records {
                journal.add((*record).clone(), machine, &origin, true);
            }
        }
        for record in &attempt {
            journal.add(record.clone(), HERE, "events/0000aa01/claims/a.json", false);
        }
        journal
    };
    let first = journal(&machines);
    let Ok(state) = replay(first.clone()) else {
        return; // a renamed copy found no free key: reported as KEY_SPACE
    };
    let said = answers(&state);
    if !placed_by_order(&read) {
        machines.reverse();
        let other = replay(journal(&machines)).expect("replay is alike in any order");
        if let Some(difference) = first_difference(&said, &answers(&other)) {
            panic!("the journals read in another order answer otherwise: {difference}");
        }
    }

    let mut export = String::new();
    for record in state.export_records() {
        let line = json::write(&Value::from(record), Format::COMPACT).expect("export is written");
        export.push_str(&line);
        export.push('\n');
    }
    if let Err(error) = read_records(&export, "export.jsonl") {
        panic!("bead's own export is refused: {error}");
    }

    let projection = project(first).expect("a projection is replay");
    let (encoded, _) = projection.encode(0);
    let mut decoded = Projection::decode(&encoded).expect("a projection decodes");
    let state = decoded.state_mut();
    for (uid, place) in state.all_wanted_texts() {
        let at = usize::try_from(place.at).expect("a place in memory");
        let bytes = &encoded.texts[at..at + place.len as usize];
        state
            .read_texts(uid, place, bytes)
            .expect("texts read back");
    }
    if let Some(difference) = first_difference(&said, &answers(decoded.state())) {
        panic!("the projection decoded answers otherwise: {difference}");
    }
});
