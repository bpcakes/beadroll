//! `FormatPolicy`: format 1 is frozen and changes are additive, so what a newer bead writes
//! travels untouched through one that cannot apply it.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::PathBuf;

use beadroll_core::json::{Format, Value, write};
use beadroll_store::{Clock, uuid7};
use beadroll_test_support::{Machine, json, parse_json};

/// The first segment under the store's `events/`, as `glob("*/*.jsonl")` finds it.
fn first_segment(machine: &Machine) -> PathBuf {
    let events = machine.store().join("events");
    std::fs::read_dir(events)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .flat_map(|replica| std::fs::read_dir(replica).unwrap())
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(".jsonl")
        })
        .expect("a segment in the store")
}

/// AF02: legacy booleans keep their JSON type, but both human views use numeric priorities.
#[test]
fn af02_list_and_show_print_legacy_boolean_priorities_as_numbers() {
    for (flag, number) in [(true, "1"), (false, "0")] {
        let sandbox = crate::sandbox();
        let m = crate::rust_only(&sandbox, "m");
        m.bead(["init", "--prefix", "ex"]);
        let created = m.bead(["create", "legacy priority"]).unwrap();
        let key = created["id"].as_str().unwrap();
        let path = first_segment(&m);
        let original = std::fs::read_to_string(&path).unwrap();
        let legacy: String = original
            .lines()
            .map(|line| {
                let mut event = parse_json(line).unwrap();
                if event["op"].as_str() == Some("create") {
                    let Value::Object(body) = &mut event else {
                        unreachable!()
                    };
                    let Some(Value::Object(data)) = body.get_mut("data") else {
                        unreachable!()
                    };
                    let Some(Value::Object(fields)) = data.get_mut("fields") else {
                        unreachable!()
                    };
                    fields.insert("priority".into(), Value::Bool(flag));
                }
                write(&event, Format::ASCII).unwrap() + "\n"
            })
            .collect();
        std::fs::write(&path, legacy).unwrap();
        let listed = m.run(["list", "--color", "never"]);
        let shown = m.run(["show", key, "--color", "never"]);
        assert_eq!(listed.returncode(), 0, "{}", listed.stderr);
        assert_eq!(shown.returncode(), 0, "{}", shown.stderr);
        assert!(
            listed.stdout.contains(&format!("[P{number}]")),
            "{}",
            listed.stdout
        );
        assert!(
            shown.stdout.contains(&format!("[P{number} · OPEN]")),
            "{}",
            shown.stdout
        );
        let detail = m.bead(["show", key]).unwrap();
        assert_eq!(detail[0]["priority"], Value::Bool(flag));
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);
    }
}

/// An event this bead cannot apply (an unknown operation, a newer format: what a newer bead would
/// write) is set aside and reported on every machine, carried through sync byte for byte, and
/// holds up nothing else; so machines upgrade in any order and a mixed fleet is safe.
#[test]
fn unapplicable_events_travel_untouched_and_hold_nothing_up() {
    let sandbox = crate::sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    a.bead(["init", "--prefix", "ex"]);
    let x = a.bead(["create", "x"]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    a.bead(["sync"]);
    b.bead(["sync"]);
    let segment = first_segment(&a);
    let text = std::fs::read_to_string(&segment).unwrap();
    // x's create: its id is the issue's uid
    let last = parse_json(text.lines().last().unwrap()).unwrap();
    let (uid, replica) = (&last["id"], &last["replica"]);
    let clock = Clock::system();
    let archive = uuid7(&clock);
    let future = [
        json!({"v": 1, "id": (archive.as_str()), "replica": (replica.clone()),
               "deps": [(uid.clone())], "op": "archive", "issue": (uid.clone()),
               "data": {"until": "2030-01-01"}}),
        json!({"v": 2, "id": (uuid7(&clock)), "replica": (replica.clone()),
               "deps": [(archive.as_str())], "op": "set", "issue": (uid.clone()),
               "data": {"fields": {"title": "from the future"}}}),
    ];
    let foreign: String = future
        .iter()
        .map(|event| write(event, Format::ASCII).unwrap() + "\n")
        .collect();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&segment)
        .unwrap();
    file.write_all(foreign.as_bytes()).unwrap();
    drop(file);
    // a normal write on top of events this bead does not understand
    a.bead(["update", &x, "--title", "after"]);
    a.bead(["sync"]);
    b.bead(["sync"]);
    for m in [&a, &b] {
        let info = m.bead(["info"]).unwrap();
        let mut problems: Vec<String> = info["integrity"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["problem"].as_str().unwrap().chars().take(17).collect())
            .collect();
        problems.sort();
        assert_eq!(
            problems,
            ["event format v2 i", "unknown operation"],
            "{}",
            m.name()
        );
        assert_eq!(m.show(&x)["title"].as_str(), Some("after"), "{}", m.name());
        assert_eq!(m.ready(), BTreeSet::from([x.clone()]), "{}", m.name());
    }
    let copy = b
        .store()
        .join("events")
        .join(replica.as_str().unwrap())
        .join(segment.file_name().unwrap());
    // carried untouched, to be applied by a bead that knows them
    assert!(std::fs::read_to_string(copy).unwrap().contains(&foreign));
    assert_eq!(a.export(), b.export());
}
