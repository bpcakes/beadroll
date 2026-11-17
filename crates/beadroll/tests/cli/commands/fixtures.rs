//! Fixtures the command modules share: a machine with a tracker, an issue's operations, the files
//! of a store, and Beads records to import.

use std::path::{Path, PathBuf};

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

/// The entries of `dir` whose names end with `suffix`, in directory order:
/// none when `dir` is not there.
pub(crate) fn glob(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| {
            let name = path.file_name().expect("a name");
            name.as_encoded_bytes().ends_with(suffix.as_bytes())
        })
        .collect()
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

/// `COMMON`: the fields every Beads record these tests import carries.
pub(crate) fn common() -> Json {
    json!({
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed",
    })
}

/// The Beads record `{"id": KEY, "title": TITLE, **COMMON}`, then the members of `more`.
pub(crate) fn record(key: &str, title: &str, more: Json) -> Json {
    let mut members = Object::new();
    members.insert("id".into(), Json::from(key));
    members.insert("title".into(), Json::from(title));
    for part in [common(), more] {
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

/// One Beads record as text: `head`, [`common`] and `tail` merged in that order (a member already
/// there keeps its place and takes the new value), written spaced and ASCII-escaped.
pub(crate) fn record_text(head: Json, tail: Json) -> String {
    let mut members = Object::new();
    for part in [head, common(), tail] {
        let Json::Object(part) = part else {
            panic!("members, not {part:?}");
        };
        members.extend(*part);
    }
    write(&Json::from(members), Format::ASCII).expect("a record")
}

/// A private-use stand-in for a lone surrogate, which a Rust string cannot hold.
pub(crate) const STAND_IN: char = '\u{E000}';

/// Member `key` of `value`; its absence fails the test.
pub(crate) fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}

/// `value` as spaced, ASCII-escaped JSON, as bead writes a record.
pub(crate) fn ascii_json(value: &Json) -> String {
    write(value, Format::ASCII).expect("JSON bead can write")
}
