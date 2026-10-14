//! What an import would do, for `--dry-run`: every problem it would be refused for, where the import
//! itself stops at the first, and what it would bring when it goes through.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::json::{self, Value};
use crate::state::State;
use crate::text::{repr_str, strip};

use super::{
    ImportError, check, create_data, create_problem, created_keys, dependencies, fail, id_of,
    resolves, target,
};

/// What an import of records (read by [`super::read_records`]) into a state would bring.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preview {
    /// The records in the file.
    pub records: usize,
    /// Those the tracker already has: an import leaves them as they are, but for dependencies they
    /// no longer list, which it takes back.
    pub known: usize,
    /// How many of the new records have each status (`open` when a record gives none).
    pub statuses: BTreeMap<String, usize>,
    /// The new records that are `in_progress`: in bead that is a claim, and they arrive with none.
    pub unowned: Vec<String>,
    /// Keys the new records depend on that neither the tracker nor the file has, each once: kept by
    /// key until an issue of that key arrives.
    pub waiting: Vec<String>,
}

/// What importing `records` into `state` would bring.
pub fn preview(state: &State, records: &[Value]) -> Preview {
    let known = state.keys();
    let ids: HashSet<&str> = records.iter().map(id_of).collect();
    let mut preview = Preview {
        records: records.len(),
        ..Preview::default()
    };
    let mut waiting: HashSet<&str> = HashSet::new();
    for record in records {
        if known.contains_key(id_of(record)) {
            preview.known += 1;
            continue;
        }
        let status = record
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("open");
        *preview.statuses.entry(status.to_owned()).or_default() += 1;
        if status == "in_progress" {
            preview.unowned.push(id_of(record).to_owned());
        }
        for dep in dependencies(record) {
            let goal = target(dep);
            if !known.contains_key(goal) && !ids.contains(goal) && waiting.insert(goal) {
                preview.waiting.push(goal.to_owned());
            }
        }
    }
    preview
}

/// Every problem an import of `text` (the file named `path`) into `state` would be refused for,
/// where the import stops at the first: rows that are not JSON, records [`super::read_records`]
/// refuses, repeated ids, and records that cannot be created. Empty exactly when
/// [`super::read_records`] and [`super::plan_import`] both accept the file. Records are numbered as
/// they are, rows of nothing but whitespace not counted.
pub fn problems(state: &State, text: &str, path: &str) -> Vec<ImportError> {
    let mut problems = Vec::new();
    let mut accepted: Vec<(usize, Value)> = Vec::new();
    let rows = text.split('\n').filter(|row| !strip(row).is_empty());
    for (number, row) in (1..).zip(rows) {
        match json::parse(row.as_bytes()) {
            Err(error) => {
                problems.push(fail(format!("{path}:{number} is not valid JSONL: {error}")))
            }
            Ok(parsed) => match check(path, number, &parsed) {
                Err(problem) => problems.push(problem),
                Ok(()) => accepted.push((number, parsed.value)),
            },
        }
    }
    let mut first: HashMap<&str, usize> = HashMap::new();
    for (number, record) in &accepted {
        let id = id_of(record);
        let earlier = *first.entry(id).or_insert(*number);
        if earlier != *number {
            problems.push(fail(format!(
                "{path}:{number}: record id {} repeats record {earlier}",
                repr_str(id)
            )));
        }
    }
    let known = state.keys();
    let fresh: Vec<&Value> = accepted
        .iter()
        .map(|(_, record)| record)
        .filter(|record| !known.contains_key(id_of(record)))
        .collect();
    let new_keys = created_keys(&fresh);
    let taken = |key: &str| resolves(known, &new_keys, key);
    for record in &fresh {
        problems.extend(create_problem(record, &create_data(record, &taken)));
    }
    problems
}
