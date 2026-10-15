//! Import planning (roll-lqjj.15): a dependency's key is looked up, never found by scanning the
//! batch, so the keys read grow with the records and dependencies, not their product.

use crate::import::dry_run::problems;
use crate::import::{plan_import, read_records};
use crate::json::{Format, Object, Value, write};

use super::super::Work;
use super::{History, dependency, grows_linearly, key};

/// The text of an export of `records` records, each depending on the key of the last, the last on
/// the first: the forward reference a scan of the batch finds last (roll-lqjj.15).
fn export(records: usize) -> String {
    let mut text = String::new();
    for n in 0..records {
        let mut record = Object::new();
        record.insert("id".into(), Value::from(key(n)));
        record.insert("title".into(), Value::from(format!("issue {n}")));
        let to = if n + 1 == records { 0 } else { records - 1 };
        let dep = Value::from(dependency(&key(to), "blocks"));
        record.insert("dependencies".into(), Value::Array(vec![dep]));
        text.push_str(&write(&Value::from(record), Format::COMPACT).unwrap());
        text.push('\n');
    }
    text
}

#[test]
fn planning_an_import_reads_each_key_a_bounded_number_of_times() {
    let state = History::new().state();
    grows_linearly(
        "planning an import whose records depend on its last",
        &[Work::Key],
        |records| read_records(&export(records), "issues.jsonl").unwrap(),
        |records| drop(plan_import(&state, &records).unwrap()),
    );
    grows_linearly(
        "validating it for a dry run",
        &[Work::Key],
        export,
        |text| assert!(problems(&state, &text, "issues.jsonl").is_empty()),
    );
}

#[test]
fn re_importing_an_issue_whose_dependencies_all_wait_reads_each_key_a_bounded_number_of_times() {
    let build = |waiting: usize| {
        let keys: Vec<(String, &str)> = (0..waiting)
            .map(|n| (format!("ex-d{n}"), "blocks"))
            .collect();
        let mut history = History::new();
        history.create("ex-known", &keys, &[]);
        let mut record = Object::new();
        record.insert("id".into(), Value::from("ex-known"));
        let listed = (keys.iter())
            .map(|(named, kind)| Value::from(dependency(named, kind)))
            .collect();
        record.insert("dependencies".into(), Value::Array(listed));
        (history.state(), vec![Value::from(record)])
    };
    grows_linearly(
        "re-importing one issue that still lists all its waiting dependencies",
        &[Work::Key],
        build,
        |(state, records)| assert!(plan_import(&state, &records).unwrap().steps.is_empty()),
    );
}
