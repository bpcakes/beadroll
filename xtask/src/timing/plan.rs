//! `timing --plan N`: what an import's core calls take on the dependency shape that scanning the
//! batch was worst for (roll-lqjj.15) — N lean records, each depending on a key at the end of the
//! batch, so a forward reference is found as late as it can be. It measures planning the import
//! into a tracker that holds none of them ([`plan_import`]), validating the same file for a dry
//! run ([`problems`], which reads the file itself, so its figure includes parsing), and planning a
//! re-import of one known issue whose N unresolved dependencies the record still lists, so that
//! every one of them is looked up and none is taken back. The fastest of [`RUNS`] runs counts.
//! Evidence, never a gate.

use std::fmt::Write as _;
use std::time::Instant;

use beadroll_core::import::dry_run::problems;
use beadroll_core::import::{plan_import, read_records};
use beadroll_core::journal::Journal;
use beadroll_core::json::{Format, Object, Value, write};
use beadroll_core::replay::replay;
use beadroll_core::state::State;

use super::world::profile;
use crate::workspace::{Failure, json_string};

/// How many times each call is measured; the fastest run counts.
const RUNS: usize = 5;

/// The file the import reads.
const PATH: &str = "issues.jsonl";

/// The id of the probe's event number `n`: a UUIDv7, ids in the order of their numbers.
fn id(n: u64) -> String {
    let millis = 1_791_558_245_123 + n;
    format!(
        "{:08x}-{:04x}-7000-8000-{:012x}",
        millis >> 16,
        millis & 0xffff,
        n
    )
}

/// An event as `Tx.emit` writes it.
fn event(n: u64, op: &str, issue: &str, data: Value) -> Value {
    let mut actor = Object::new();
    actor.insert("agent".into(), Value::from("timing"));
    actor.insert("thread".into(), Value::from("a"));
    let mut members = Object::new();
    members.insert("v".into(), Value::from(1));
    members.insert("id".into(), Value::from(id(n)));
    members.insert("replica".into(), Value::from("a"));
    let deps: Vec<Value> = (1..n).map(|earlier| Value::from(id(earlier))).collect();
    members.insert("deps".into(), Value::Array(deps));
    members.insert("actor".into(), Value::from(actor));
    members.insert("op".into(), Value::from(op));
    members.insert("issue".into(), Value::from(issue));
    members.insert("data".into(), data);
    Value::from(members)
}

/// A dependency on `key`, as a record lists one and a create carries an unresolved one.
fn dependency(key: &str) -> Value {
    let mut dep = Object::new();
    dep.insert("depends_on_id".into(), Value::from(key));
    dep.insert("type".into(), Value::from("blocks"));
    Value::from(dep)
}

/// The key of the issue a re-import's record is.
const KNOWN: &str = "ex-known";

/// The tracker the probe imports into: one issue, `dangling` dependencies on `waiting` keys that
/// no issue has, as an import of records naming keys out of order leaves them.
fn tracker(waiting: usize) -> Result<State, Failure> {
    let mut data = Object::new();
    data.insert("prefix".into(), Value::from("ex"));
    let mut create = Object::new();
    create.insert("key".into(), Value::from(KNOWN));
    let mut fields = Object::new();
    fields.insert("title".into(), Value::from("known"));
    create.insert("fields".into(), Value::from(fields));
    create.insert("imported".into(), Value::Bool(true));
    let dangling: Vec<Value> = (0..waiting)
        .map(|n| dependency(&format!("ex-d{n}")))
        .collect();
    if !dangling.is_empty() {
        create.insert("dangling".into(), Value::Array(dangling));
    }
    let mut journal = Journal::new();
    for event in [
        event(1, "init", &id(1), Value::from(data)),
        event(2, "create", &id(2), Value::from(create)),
    ] {
        journal.add(event, "a", PATH, true);
    }
    replay(journal).map_err(|_| Failure::Failed("the probe's own tracker does not replay".into()))
}

/// An export of `records` records, each depending on the key of the last one: the forward
/// reference a scan of the batch finds last. The last record depends on the first instead, as no
/// record may depend on itself.
fn export(records: usize) -> String {
    let mut out = String::new();
    for n in 0..records {
        let mut record = Object::new();
        record.insert("id".into(), Value::from(format!("ex-{n}")));
        record.insert("title".into(), Value::from(format!("issue {n}")));
        let last = records.saturating_sub(1);
        let to = if n == last { 0 } else { last };
        if records > 1 {
            record.insert(
                "dependencies".into(),
                Value::Array(vec![dependency(&format!("ex-{to}"))]),
            );
        }
        let line = write(&Value::from(record), Format::COMPACT).unwrap_or_default();
        let _ = writeln!(out, "{line}");
    }
    out
}

/// A re-import of the known issue: one record listing every key the tracker still waits for, so
/// planning looks each unresolved dependency up and takes none back.
fn reimport(waiting: usize) -> String {
    let mut record = Object::new();
    record.insert("id".into(), Value::from(KNOWN));
    record.insert("title".into(), Value::from("known"));
    let listed: Vec<Value> = (0..waiting)
        .map(|n| dependency(&format!("ex-d{n}")))
        .collect();
    record.insert("dependencies".into(), Value::Array(listed));
    let line = write(&Value::from(record), Format::COMPACT).unwrap_or_default();
    format!("{line}\n")
}

/// The fastest of [`RUNS`] runs of `call`, and what its last run answered.
fn fastest<T>(mut call: impl FnMut() -> Result<T, Failure>) -> Result<(f64, T), Failure> {
    let mut best = f64::INFINITY;
    let mut answer = None;
    for _ in 0..RUNS {
        let clock = Instant::now();
        let outcome = call()?;
        best = best.min(clock.elapsed().as_secs_f64());
        answer = Some(outcome);
    }
    match answer {
        Some(answer) => Ok((best, answer)),
        None => Err(Failure::Failed("a timing runs at least once".into())),
    }
}

/// Measures; returns the beginning of the line to record.
pub(crate) fn measure(records: usize, label: Option<&str>) -> Result<String, Failure> {
    let state = tracker(0)?;
    let text = export(records);
    let bytes = text.len();
    let parsed = read_records(&text, PATH)
        .map_err(|error| Failure::Failed(format!("the probe's own export is refused: {error}")))?;
    let (planning, plan) = fastest(|| {
        plan_import(&state, &parsed)
            .map_err(|error| Failure::Failed(format!("planning failed: {error}")))
    })?;
    let (validation, found) = fastest(|| Ok(problems(&state, &text, PATH).len()))?;
    drop(text);
    drop(parsed);
    // The same number of dependencies again, all of them unresolved and all still listed.
    let waiting = tracker(records)?;
    let again = read_records(&reimport(records), PATH)
        .map_err(|error| Failure::Failed(format!("the probe's own re-import: {error}")))?;
    let (reimporting, retractions) = fastest(|| {
        let plan = plan_import(&waiting, &again)
            .map_err(|error| Failure::Failed(format!("re-planning failed: {error}")))?;
        Ok(plan.steps.len())
    })?;
    let label = label.map_or_else(
        || format!("import plan of {records} records"),
        str::to_owned,
    );
    Ok(format!(
        "{{\"label\": {}, \"records\": {records}, \"export_bytes\": {bytes}, \"profile\": {}, \
         \"runs\": {RUNS}, \"steps\": {}, \"imported\": {}, \"plan_seconds\": {planning:.4}, \
         \"problems\": {found}, \"validate_seconds\": {validation:.4}, \
         \"retractions\": {retractions}, \"reimport_seconds\": {reimporting:.4}, ",
        json_string(&label),
        json_string(profile()),
        plan.steps.len(),
        plan.imported,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_record_depends_on_a_key_at_the_end_of_the_batch() {
        let text = export(4);
        let records = read_records(&text, PATH).unwrap();
        assert_eq!(records.len(), 4);
        let targets: Vec<&str> = records
            .iter()
            .map(|record| {
                record
                    .get("dependencies")
                    .and_then(Value::as_array)
                    .unwrap()[0]
                    .get("depends_on_id")
                    .and_then(Value::as_str)
                    .unwrap()
            })
            .collect();
        assert_eq!(targets, ["ex-3", "ex-3", "ex-3", "ex-0"]);
        assert_eq!(
            export(1).lines().count(),
            1,
            "one record depends on nothing"
        );
    }

    #[test]
    fn the_plan_creates_every_record_and_resolves_every_dependency() {
        let state = tracker(0).unwrap();
        let text = export(8);
        let plan = plan_import(&state, &read_records(&text, PATH).unwrap()).unwrap();
        assert_eq!(plan.imported, 8);
        assert_eq!(plan.steps.len(), 16, "eight creates and eight additions");
        assert!(problems(&state, &text, PATH).is_empty());
    }

    #[test]
    fn a_reimport_looks_every_unresolved_dependency_up_and_takes_none_back() {
        let waiting = tracker(6).unwrap();
        let state = waiting.issues().next().unwrap();
        assert_eq!(state.dangling().len(), 6);
        let text = reimport(6);
        let plan = plan_import(&waiting, &read_records(&text, PATH).unwrap()).unwrap();
        assert_eq!(plan.imported, 0);
        assert!(plan.steps.is_empty(), "every key is still listed: {plan:?}");
        // One the record no longer lists is taken back: that the lookup answers both ways.
        let fewer = reimport(5);
        let plan = plan_import(&waiting, &read_records(&fewer, PATH).unwrap()).unwrap();
        assert_eq!(plan.steps.len(), 1, "{plan:?}");
    }
}
