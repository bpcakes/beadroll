//! Fixtures the command modules share: a machine with a tracker, an issue's operations, the files
//! of a store, and Beads records to import.

use std::path::PathBuf;

use beadroll_core::json::{Format, Object, write};
use beadroll_test_support::{Json, Machine, Sandbox, json, parse_json};

/// A machine `name` whose tracker is initialised, prefix `ex`.
pub(crate) fn machine(sandbox: &Sandbox, name: &str) -> Machine {
    let machine = Machine::new(sandbox, name);
    machine.bead(["init", "--prefix", "ex"]);
    machine
}

/// `ops`: the operation of each of `key`'s events, as `bead log KEY --limit 0` lists them.
pub(crate) fn ops(machine: &Machine, key: &str) -> Vec<String> {
    let log = machine.bead(["log", key, "--limit", "0"]).expect("a log");
    let events = log.as_array().expect("bead log prints a list");
    events
        .iter()
        .map(|event| event["op"].as_str().expect("an op").to_owned())
        .collect()
}

/// The number of occurrences of `op` in `ops`.
pub(crate) fn count(ops: &[String], op: &str) -> usize {
    ops.iter().filter(|each| *each == op).count()
}

/// The `meta.json` of each store in `machine`'s home, in directory order.
pub(crate) fn metas(machine: &Machine) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(machine.home()) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.expect("a directory entry").path().join("meta.json"))
        .filter(|path| path.exists())
        .collect()
}

/// The fields every Beads record these tests import carries besides its id and title: an open
/// task of priority 2, created and updated at the start of 2026 by `seed`.
pub(crate) fn record_fields() -> Json {
    json!({
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed",
    })
}

/// The Beads record with id `key` and title `title`, then [`record_fields`], then the members of
/// `more`.
pub(crate) fn record(key: &str, title: &str, more: Json) -> Json {
    let mut members = Object::new();
    members.insert("id".into(), Json::from(key));
    members.insert("title".into(), Json::from(title));
    for part in [record_fields(), more] {
        match part {
            Json::Object(part) => members.extend(*part),
            other => panic!("{other:?} is not an object"),
        }
    }
    Json::from(members)
}

/// Records as spaced, ASCII-escaped JSON, each followed by a newline.
pub(crate) fn jsonl(records: &[Json]) -> String {
    records
        .iter()
        .map(|record| write(record, Format::ASCII).expect("a record bead can read") + "\n")
        .collect()
}

/// Reads strict JSON or fails the test. NaN, infinity and numbers overflowing a float are
/// refused.
pub(crate) fn strict_json(text: &str) -> Json {
    parse_json(text).unwrap_or_else(|error| panic!("not strict JSON ({error}): {text:?}"))
}

/// One Beads record as text: `head`, [`record_fields`] and `tail` merged in that order (a member already
/// there keeps its place and takes the new value), written spaced and ASCII-escaped.
pub(crate) fn record_text(head: Json, tail: Json) -> String {
    let mut members = Object::new();
    for part in [head, record_fields(), tail] {
        let Json::Object(part) = part else {
            panic!("members, not {part:?}");
        };
        members.extend(*part);
    }
    write(&Json::from(members), Format::ASCII).expect("a record")
}

/// A private-use stand-in for a lone surrogate, which a Rust string cannot hold.
pub(crate) const STAND_IN: char = '\u{E000}';

/// `value` as spaced, ASCII-escaped JSON, as bead writes a record.
pub(crate) fn ascii_json(value: &Json) -> String {
    write(value, Format::ASCII).expect("JSON bead can write")
}

/// Machine `root2-a` with tracker `re` initialised, one issue created, and synced: the machine and
/// the issue's id.
pub(crate) fn published_issue(sandbox: &Sandbox) -> (Machine, String) {
    let a = Machine::new(sandbox, "root2-a");
    a.bead(["init", "--prefix", "re"]);
    let created = a.bead(["create", "root re-audit"]).expect("the issue");
    let key = created["id"].as_str().expect("an id").to_owned();
    a.bead(["sync"]);
    (a, key)
}
