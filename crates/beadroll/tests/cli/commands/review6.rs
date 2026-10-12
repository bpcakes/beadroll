//! `Review6`: the sixth independent review's findings (F01 to F16, `docs/FINDINGS.md`) and the
//! properties it verified, one module per subject. F09's replay half is ported in beadroll-core
//! (`tests/core/commands.rs`); F14 and the clock-skew test, in `clock.rs`, set the Rust `bead`'s
//! clock with its `clock` failpoint.

use std::path::{Path, PathBuf};

use beadroll_core::json::{Format, Object, write};
use beadroll_test_support::{Json, Machine, Sandbox, json};

mod clock;
mod git_helper;
mod input;
mod output;
mod receipts;
mod remotes;
mod store;

/// `self.machine(name)`: a machine `name` whose tracker is initialised, prefix `ex`.
fn machine(sandbox: &Sandbox, name: &str) -> Machine {
    let machine = Machine::new(sandbox, name);
    machine.bead(["init", "--prefix", "ex"]);
    machine
}

/// The id of the issue a `--json` result is: `result["id"]`.
fn id(result: Option<Json>) -> String {
    let result = result.expect("bead printed a result");
    result["id"].as_str().expect("an id").to_owned()
}

/// `ops`: the operation of each of `key`'s events, as `bead log KEY --limit 0` lists them.
fn ops(machine: &Machine, key: &str) -> Vec<String> {
    let log = machine.bead(["log", key, "--limit", "0"]).expect("a log");
    let events = log.as_array().expect("bead log prints a list");
    events
        .iter()
        .map(|event| event["op"].as_str().expect("an op").to_owned())
        .collect()
}

/// How many times `op` is in `ops`: Python's `list.count`.
fn count(ops: &[String], op: &str) -> usize {
    ops.iter().filter(|each| *each == op).count()
}

/// The entries of `dir` whose names end with `suffix`, in directory order, as
/// `Path.glob("*SUFFIX")` lists them: none when `dir` is not there.
fn glob(dir: &Path, suffix: &str) -> Vec<PathBuf> {
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

/// The `meta.json` of each store in `machine`'s home, in directory order:
/// `machine.home.glob("*/meta.json")`.
fn metas(machine: &Machine) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(machine.home()) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.expect("a directory entry").path().join("meta.json"))
        .filter(|path| path.exists())
        .collect()
}

/// `COMMON`: the fields every Beads record these tests import carries.
fn common() -> Json {
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
fn record(key: &str, title: &str, more: Json) -> Json {
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

/// `records` as a JSONL file's text, each line as Python's `json.dumps(record) + "\n"` writes it.
fn jsonl(records: &[Json]) -> String {
    records
        .iter()
        .map(|record| write(record, Format::ASCII).expect("a record bead can read") + "\n")
        .collect()
}
