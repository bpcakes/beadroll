//! Beads JSONL records exported from state for bv and other Beads readers.

use std::collections::{BTreeMap, HashSet};

use sha2::{Digest, Sha256};

use crate::event::{EXPORT_FIELDS, Field, status};
use crate::id::EventId;
use crate::json::{Int, Object, Value};
use crate::op::IntegerField;
use crate::state::{Comment, Issue, State};

/// Where the ids of comments made here start: 2^47, clear of the small ids Beads hands out.
pub(crate) const COMMENT_ID_BASE: u64 = 1 << 47;

/// The members of a record that are bead's own: an extra field never overwrites one on export, and
/// an import keeps none of them among a create's extras.
pub(crate) fn reserved(name: &str) -> bool {
    EXPORT_FIELDS.iter().any(|field| field.name() == name)
        || matches!(
            name,
            "id" | "labels" | "dependencies" | "comments" | "source_repo_path"
        )
}

/// `value` of `field` as a Beads reader takes it, and bead's own import, which refuses what
/// `event::write_problem` refuses: a boolean priority or estimate as its number (`true` is 1, as
/// `list` and `show` print it), and an estimate outside 0..=2147483647 left out. Histories written
/// before those writes were refused hold them, and replay keeps them as they were.
fn beads_readable(field: Field, value: &Value) -> Option<Value> {
    let integer = || IntegerField::read(value).map(|integer| Value::Int(integer.number()));
    match field {
        Field::Priority => Some(integer().unwrap_or_else(|| value.clone())),
        Field::EstimatedMinutes if *value != Value::Null => {
            let minutes = integer()?;
            let fits = matches!(&minutes, Value::Int(m) if Int::from(0) <= *m && *m <= Int::from(2_147_483_647));
            fits.then_some(minutes)
        }
        _ => Some(value.clone()),
    }
}

/// The members a delete writes among an issue's extras: a record exports them only while the issue
/// is a tombstone, never after a reopen or another status left it alive.
const DELETION: [&str; 3] = ["deleted_at", "deleted_by", "delete_reason"];

/// The id a comment made here exports: derived from its event id, so every machine and every later
/// export agree on it.
pub(crate) fn comment_id(uid: EventId) -> u64 {
    let digest = Sha256::digest(uid.to_string().as_bytes());
    let number = digest
        .iter()
        .take(6)
        .fold(0u64, |number, byte| number << 8 | u64::from(*byte));
    COMMENT_ID_BASE + number % COMMENT_ID_BASE
}

/// An imported comment keeps its id, unless it collides with one already handed out, a derived one
/// of ours or another imported comment's: it counts on to the next free, and the id it gets is then
/// taken, so that no two comments export one id.
fn exported_legacy_id(comment: &Object, taken: &mut HashSet<u64>) -> Value {
    let number = comment.get("legacy_id").cloned().unwrap_or(Value::Null);
    let Value::Int(mut id) = number else {
        return number;
    };
    let collides = |id: &Int| {
        id.as_i64()
            .and_then(|id| u64::try_from(id).ok())
            .is_some_and(|id| taken.contains(&id))
    };
    while collides(&id) {
        // Only an id within an i64 collides, so counting on from one never runs out.
        let Some(next) = id.plus_one() else {
            break;
        };
        id = next;
    }
    if let Some(number) = id.as_i64().and_then(|id| u64::try_from(id).ok()) {
        taken.insert(number);
    }
    Value::Int(id)
}

impl State {
    /// Beads-compatible records, one per issue, by key.
    pub(crate) fn export_records(&self) -> Vec<Object> {
        // A comment made here exports an integer derived from its event id. The range (2^47 upwards,
        // 48 bits) makes two comments sharing a number a one-in-10^14 event; should it happen, the tie
        // is broken by the ids alone, never by arrival order. An imported id is kept as it came unless
        // it collides with one of these or with an imported one handed out before it, in key and
        // then comment order, so every machine moves the same one.
        let mut ours: Vec<EventId> = self
            .issues()
            .flat_map(|i| i.comments().iter().filter_map(Comment::uid))
            .collect();
        ours.sort_unstable();
        let (mut ids, mut taken) = (BTreeMap::new(), HashSet::new());
        for uid in ours {
            let mut number = comment_id(uid);
            while taken.contains(&number) {
                number += 1;
            }
            taken.insert(number);
            ids.insert(uid, number);
        }
        let mut ids = CommentIds { ids, taken };
        self.issues_by_key()
            .into_iter()
            .map(|issue| self.record(issue, &mut ids))
            .collect()
    }

    /// One issue's record.
    fn record(&self, issue: &Issue, ids: &mut CommentIds) -> Object {
        // Extras first, never over the record.
        let deleted = issue.status() == status::TOMBSTONE;
        let mut rec: Object = issue
            .extra()
            .iter()
            .filter(|(name, _)| !reserved(name))
            .filter(|(name, _)| deleted || !DELETION.contains(&name.as_str()))
            .map(|(n, v)| (n.clone(), v.clone()))
            .collect();
        rec.insert("id".into(), Value::from(issue.key()));
        for field in EXPORT_FIELDS {
            if let Some(value) = issue
                .get(field)
                .and_then(|value| beads_readable(field, value))
            {
                rec.insert(field.name().into(), value);
            }
        }
        if !issue.labels().is_empty() {
            let labels: Vec<Value> = issue
                .labels()
                .iter()
                .map(|l| Value::from(l.as_str()))
                .collect();
            rec.insert("labels".into(), labels.into());
        }
        let dependencies = self.dependencies(issue);
        if !dependencies.is_empty() {
            rec.insert("dependencies".into(), dependencies.into());
        }
        if !issue.comments().is_empty() {
            let comments = issue.comments().iter().map(|comment| {
                let members = comment.to_value();
                let id = match comment.uid().and_then(|uid| ids.ids.get(&uid)) {
                    Some(number) => Value::Int(Int::from_u64(*number)),
                    None => exported_legacy_id(&members, &mut ids.taken),
                };
                let mut out = Object::new();
                out.insert("id".into(), id);
                out.insert("issue_id".into(), Value::from(issue.key()));
                for name in ["author", "text", "created_at"] {
                    out.insert(
                        name.into(),
                        members.get(name).cloned().unwrap_or(Value::Null),
                    );
                }
                Value::from(out)
            });
            rec.insert("comments".into(), comments.collect::<Vec<_>>().into());
        }
        rec
    }

    /// A record's dependencies, by target then type: its edges, and the imported dependencies on
    /// keys that are still unknown, which travel on as they were imported under this record's
    /// (possibly renamed) id.
    fn dependencies(&self, issue: &Issue) -> Vec<Value> {
        let deps = issue.deps().iter().map(|(edge, meta)| {
            // read in with the issue's texts, as an export reads every issue's
            let member = |name: &str, default: Value| {
                (meta.members())
                    .and_then(|members| members.get(name).cloned())
                    .unwrap_or(default)
            };
            let mut dep = Object::new();
            dep.insert("issue_id".into(), Value::from(issue.key()));
            dep.insert(
                "depends_on_id".into(),
                Value::from(&*self.target_key(edge.to())),
            );
            dep.insert("type".into(), Value::from(edge.kind()));
            dep.insert("created_at".into(), member("created_at", Value::Null));
            dep.insert("created_by".into(), member("created_by", Value::Null));
            dep.insert("metadata".into(), member("metadata", Value::from("{}")));
            dep.insert("thread_id".into(), member("thread_id", Value::from("")));
            dep
        });
        let dangling = issue.dangling().iter().map(|dep| {
            let mut dep = dep.as_object().cloned().unwrap_or_default();
            dep.insert("issue_id".into(), Value::from(issue.key()));
            dep
        });
        let mut dependencies: Vec<Object> = deps.chain(dangling).collect();
        dependencies.sort_by_key(|dep| {
            let text = |name: &str| {
                dep.get(name)
                    .filter(|v| v.truthy())
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            };
            (text("depends_on_id").to_owned(), text("type").to_owned())
        });
        dependencies.into_iter().map(Value::from).collect()
    }
}

/// The ids comments made here export, and every id they take.
struct CommentIds {
    ids: BTreeMap<EventId, u64>,
    taken: HashSet<u64>,
}

#[cfg(test)]
mod tests;
