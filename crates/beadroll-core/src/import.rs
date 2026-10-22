//! Plans a Beads JSONL import: validates records and produces ordered events. The caller writes
//! all events or none. [`dry_run`] says what an import would do without planning to write it.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use indexmap::IndexMap;

use crate::event::{EXPORT_FIELDS, MAX_NESTING, event_problem, payload_problem, write_problem};
use crate::id::EventId;
use crate::json::{self, Object, Value};
use crate::state::State;
use crate::text::{repr_str, strip};
use crate::work::{Work, tally};

pub mod dry_run;

/// The id a would-be create is checked under before it is minted.
pub const PROBE_ID: &str = "00000000-0000-7000-8000-000000000000";

/// Why the whole import fails, reported as VALIDATION.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ImportError(pub String);

/// The issue a dependency targets: one the tracker has, or one this import creates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// An issue the tracker has.
    Issue(EventId),
    /// The issue the plan's create number `n` (from 0) makes.
    Created(usize),
}

/// One event of an import, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// A known record no longer lists a dependency on a key: take it back
    /// (`dep_remove {"key", "type"}` on the issue).
    DepRemove {
        /// The issue that holds it.
        issue: EventId,
        /// Its payload.
        data: Object,
    },
    /// A new record's issue (`create`, its payload).
    Create(Object),
    /// A new record's dependency on an issue that exists, or is created here.
    DepAdd {
        /// The create, by its number among the plan's creates.
        issue: usize,
        /// What it depends on.
        to: Target,
        /// Its payload, but for `to`, which goes first.
        data: Object,
    },
}

impl Step {
    /// The operation it emits.
    pub fn op(&self) -> &'static str {
        match self {
            Step::DepRemove { .. } => "dep_remove",
            Step::Create(_) => "create",
            Step::DepAdd { .. } => "dep_add",
        }
    }
}

/// An import's events, and how many records it creates.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The events, in order.
    pub steps: Vec<Step>,
    /// The records new to the tracker.
    pub imported: usize,
}

fn fail(message: String) -> ImportError {
    ImportError(message)
}

/// The records of `text` (the file named `path`, already decoded as UTF-8), each checked: what an
/// import reads before it looks at the tracker.
pub fn read_records(text: &str, path: &str) -> Result<Vec<Value>, ImportError> {
    // Records end at "\n" only: U+2028, U+2029 and U+0085 are ordinary text inside a JSON string.
    let mut records = Vec::new();
    for (number, row) in (1..).zip(text.split('\n').filter(|row| !strip(row).is_empty())) {
        let parsed = json::parse(row.as_bytes())
            .map_err(|error| fail(format!("{path}:{number} is not valid JSONL: {error}")))?;
        records.push(parsed.value);
    }
    for (number, record) in (1..).zip(&records) {
        check(path, number, record)?;
    }
    // One id is one issue; br refuses a file that repeats one too.
    let mut first: HashMap<&str, usize> = HashMap::new();
    for (number, record) in (1..).zip(&records) {
        let id = id_of(record);
        let earlier = *first.entry(id).or_insert(number);
        if earlier != number {
            return Err(fail(format!(
                "{path}:{number}: record id {} repeats record {earlier}",
                repr_str(id)
            )));
        }
    }
    Ok(records)
}

/// The events that import `records` (read by [`read_records`]) into `state`.
pub fn plan_import(state: &State, records: &[Value]) -> Result<Plan, ImportError> {
    let known = state.keys();
    let fresh: Vec<&Value> = records
        .iter()
        .filter(|record| !known.contains_key(id_of(record)))
        .collect();
    let mut steps = Vec::new();
    for record in records {
        // A known record that no longer lists a dependency we still hold by key takes it back.
        let Some(uid) = known.get(id_of(record)) else {
            continue;
        };
        let dangling = state
            .issue(*uid)
            .map(|issue| issue.dangling())
            .unwrap_or_default();
        if dangling.is_empty() {
            // The usual re-import: the record is here, its dependencies all resolved long ago.
            continue;
        }
        let listed: HashSet<(&str, &str)> = dependencies(record)
            .iter()
            .map(|d| (target(d), kind(d)))
            .collect();
        for dep in dangling {
            if !listed.contains(&(target(dep), kind(dep))) {
                let data = members([
                    ("key", Value::from(target(dep))),
                    ("type", Value::from(kind(dep))),
                ]);
                steps.push(Step::DepRemove { issue: *uid, data });
            }
        }
    }
    // Only `fresh` determines the order of creates and dependency additions.
    let new_keys = created_keys(&fresh);
    let taken = |key: &str| resolves(known, &new_keys, key);
    for record in &fresh {
        let data = create_data(record, &taken);
        if let Some(problem) = create_problem(record, &data) {
            // Nothing has been written yet: the import fails whole.
            return Err(problem);
        }
        steps.push(Step::Create(data));
    }
    for (created, record) in fresh.iter().enumerate() {
        for dep in dependencies(record) {
            let goal = target(dep);
            let to = match (known.get(goal), new_keys.get(goal).copied()) {
                (Some(uid), _) => Target::Issue(*uid),
                (None, Some(at)) => Target::Created(at),
                (None, None) => continue,
            };
            let meta: Object = ["created_at", "created_by", "metadata", "thread_id"]
                .into_iter()
                .filter_map(|name| dep.get(name).map(|value| (name.to_owned(), value.clone())))
                .collect();
            let data = members([
                ("type", Value::from(kind(dep))),
                ("meta", Value::from(meta)),
                ("imported", Value::Bool(true)),
            ]);
            steps.push(Step::DepAdd {
                issue: created,
                to,
                data,
            });
        }
    }
    Ok(Plan {
        steps,
        imported: fresh.len(),
    })
}

/// Why `record`'s create, whose payload is `data`, cannot be written, checked as the create bead
/// would mint for it.
fn create_problem(record: &Value, data: &Object) -> Option<ImportError> {
    let mut probe = Object::new();
    probe.insert("v".into(), Value::from(1));
    probe.insert("id".into(), Value::from(PROBE_ID));
    probe.insert("replica".into(), Value::from("probe"));
    probe.insert("deps".into(), Value::Array(Vec::new()));
    probe.insert("op".into(), Value::from("create"));
    probe.insert("issue".into(), Value::from(PROBE_ID));
    probe.insert("data".into(), Value::from(data.clone()));
    let probe = Value::from(probe);
    let problem = event_problem(&probe)
        .or_else(|| payload_problem(&probe))
        .or_else(|| write_problem(&probe))?;
    Some(fail(format!(
        "record {} cannot be imported: {problem}",
        repr_str(id_of(record))
    )))
}

/// The checks every record passes before anything is planned.
fn check(path: &str, number: usize, record: &Value) -> Result<(), ImportError> {
    let Some(id) = record
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Err(fail(format!("{path}:{number}: record has no string 'id'")));
    };
    let id = repr_str(id);
    let comments = record.get("comments").filter(|c| c.truthy());
    if comments.is_some_and(|c| {
        !c.as_array()
            .is_some_and(|c| c.iter().all(|c| c.as_object().is_some()))
    }) {
        return Err(fail(format!(
            "{path}:{number}: record {id} has malformed comments"
        )));
    }
    let deps = record.get("dependencies").filter(|d| d.truthy());
    let well_formed = |dep: &Value| {
        dep.as_object().is_some()
            && dep
                .get("depends_on_id")
                .and_then(Value::as_str)
                .is_some_and(|target| !target.is_empty())
            && matches!(dep.get("type"), None | Some(Value::Null | Value::Str(_)))
    };
    if deps.is_some_and(|d| !d.as_array().is_some_and(|d| d.iter().all(well_formed))) {
        return Err(fail(format!(
            "{path}:{number}: record {id} has a malformed dependency"
        )));
    }
    if dependencies(record)
        .iter()
        .any(|dep| dep.get("depends_on_id") == record.get("id"))
    {
        // As `dep add` and br refuse it.
        return Err(fail(format!(
            "{path}:{number}: record {id} depends on itself"
        )));
    }
    // Its create event holds it two levels down (data, then extra).
    if record.deeper_than(MAX_NESTING - 2) {
        return Err(fail(format!(
            "{path}:{number}: record {id} nests more than {} levels deep",
            MAX_NESTING - 2
        )));
    }
    Ok(())
}

fn members<const N: usize>(items: [(&str, Value); N]) -> Object {
    items
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
}

fn id_of(record: &Value) -> &str {
    tally(Work::Key, 1);
    record.get("id").and_then(Value::as_str).unwrap_or_default()
}

/// A record's dependency array, or an empty slice when no array is present.
fn dependencies(record: &Value) -> &[Value] {
    record
        .get("dependencies")
        .and_then(Value::as_array)
        .unwrap_or_default()
}

/// The key a dependency names, empty when it names none.
fn target(dep: &Value) -> &str {
    tally(Work::Key, 1);
    dep.get("depends_on_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// A dependency's nonempty type string, defaulting to `blocks`.
fn kind(dep: &Value) -> &str {
    dep.get("type")
        .filter(|kind| kind.truthy())
        .and_then(Value::as_str)
        .unwrap_or("blocks")
}

/// The keys `fresh` (the records the tracker does not have) brings, each with the number of the
/// create that makes it: how a dependency finds a target in its own batch.
fn created_keys<'a>(fresh: &[&'a Value]) -> HashMap<&'a str, usize> {
    let mut keys = HashMap::with_capacity(fresh.len());
    for (created, record) in fresh.iter().enumerate() {
        keys.entry(id_of(record)).or_insert(created);
    }
    keys
}

/// Whether a dependency on `key` has an issue once the import is done: one the tracker holds, or
/// one the batch creates. Planning and [`dry_run::problems`] decide it here, together, so that the
/// payload a dry run checks is the payload the import writes.
fn resolves(known: &IndexMap<String, EventId>, created: &HashMap<&str, usize>, key: &str) -> bool {
    known.contains_key(key) || created.contains_key(key)
}

/// The members of a record that is not an object.
static NO_MEMBERS: LazyLock<Object> = LazyLock::new(Object::new);

/// What the create of a new record carries.
fn create_data(record: &Value, taken: &dyn Fn(&str) -> bool) -> Object {
    let reserved = |name: &str| {
        EXPORT_FIELDS.iter().any(|field| field.name() == name)
            || matches!(
                name,
                "id" | "labels" | "dependencies" | "comments" | "source_repo_path"
            )
    };
    let all = record.as_object().unwrap_or(&NO_MEMBERS);
    let extra: Object = all
        .iter()
        .filter(|(name, _)| !reserved(name))
        .map(|(n, v)| (n.clone(), v.clone()))
        .collect();
    let dangling: Vec<Value> = dependencies(record)
        .iter()
        .filter(|dep| !taken(target(dep)))
        .map(|dep| {
            let mut dep = dep.as_object().cloned().unwrap_or_default();
            let kind = dep
                .get("type")
                .filter(|kind| kind.truthy())
                .cloned()
                .unwrap_or(Value::from("blocks"));
            dep.insert("type".into(), kind);
            Value::from(dep)
        })
        .collect();
    let comments: Vec<Value> = record
        .get("comments")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(|comment| {
            let get = |name: &str| comment.get(name).cloned().unwrap_or(Value::Null);
            Value::from(members([
                ("legacy_id", get("id")),
                ("author", get("author")),
                (
                    "text",
                    comment.get("text").cloned().unwrap_or(Value::from("")),
                ),
                ("created_at", get("created_at")),
            ]))
        })
        .collect();
    let fields: Object = EXPORT_FIELDS
        .iter()
        .filter_map(|field| {
            all.get(field.name())
                .map(|value| (field.name().to_owned(), value.clone()))
        })
        .collect();
    let labels = all
        .get("labels")
        .filter(|labels| labels.truthy())
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let mut data = members([
        ("key", Value::from(id_of(record))),
        ("fields", Value::from(fields)),
        ("labels", labels),
        ("comments", Value::Array(comments)),
        ("extra", Value::from(extra)),
        ("imported", Value::Bool(true)),
    ]);
    if !dangling.is_empty() {
        data.insert("dangling".into(), Value::Array(dangling));
    }
    data
}
