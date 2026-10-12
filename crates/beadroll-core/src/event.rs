//! What an event must look like: the envelope every event shares (`event_problem`), the payload of
//! each operation (`payload_problem`), and the stricter rules for events this bead writes
//! (`write_problem`). Each answers with Python's message, or `None`.

use crate::id::EventId;
use crate::json::{Int, Object, Value, member};
use crate::key::number_too_long;
use crate::op::PyInt;
use crate::text::repr_str;
use crate::time::MAX_MILLIS;

/// The format of events this bead reads and writes.
pub const FORMAT: i64 = 1;

/// The operations replay applies.
pub const KNOWN_OPS: [&str; 14] = [
    "init",
    "create",
    "set",
    "claim",
    "release",
    "close",
    "reopen",
    "delete",
    "comment",
    "dep_add",
    "dep_remove",
    "label_add",
    "label_remove",
    "label_set",
];

/// The statuses bead's own commands set; Beads tools write others, kept as they are.
pub const STATUSES: [&str; 6] = [
    "open",
    "in_progress",
    "blocked",
    "deferred",
    "closed",
    "tombstone",
];

/// Statuses that count as finished.
pub const DONE: [&str; 2] = ["closed", "tombstone"];

/// Arrays and objects one event may nest when this bead writes it (Python's `MAX_NESTING`).
pub const MAX_NESTING: usize = 100;

/// The type an issue field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    /// A string.
    Str,
    /// An integer (a boolean is one, to Python).
    Int,
}

/// An issue field an event may set: Python's `FIELD_TYPES`, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Field {
    /// `title`.
    Title,
    /// `description`.
    Description,
    /// `design`.
    Design,
    /// `acceptance_criteria`.
    AcceptanceCriteria,
    /// `notes`.
    Notes,
    /// `priority`.
    Priority,
    /// `issue_type`.
    IssueType,
    /// `assignee`.
    Assignee,
    /// `estimated_minutes`.
    EstimatedMinutes,
    /// `external_ref`.
    ExternalRef,
    /// `status`.
    Status,
    /// `created_at`.
    CreatedAt,
    /// `updated_at`.
    UpdatedAt,
    /// `closed_at`.
    ClosedAt,
    /// `close_reason`.
    CloseReason,
    /// `created_by`.
    CreatedBy,
}

impl Field {
    /// Every field, in `FIELD_TYPES` order.
    pub const ALL: [Field; 16] = [
        Field::Title,
        Field::Description,
        Field::Design,
        Field::AcceptanceCriteria,
        Field::Notes,
        Field::Priority,
        Field::IssueType,
        Field::Assignee,
        Field::EstimatedMinutes,
        Field::ExternalRef,
        Field::Status,
        Field::CreatedAt,
        Field::UpdatedAt,
        Field::ClosedAt,
        Field::CloseReason,
        Field::CreatedBy,
    ];

    /// Its name in events and records.
    pub fn name(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Description => "description",
            Field::Design => "design",
            Field::AcceptanceCriteria => "acceptance_criteria",
            Field::Notes => "notes",
            Field::Priority => "priority",
            Field::IssueType => "issue_type",
            Field::Assignee => "assignee",
            Field::EstimatedMinutes => "estimated_minutes",
            Field::ExternalRef => "external_ref",
            Field::Status => "status",
            Field::CreatedAt => "created_at",
            Field::UpdatedAt => "updated_at",
            Field::ClosedAt => "closed_at",
            Field::CloseReason => "close_reason",
            Field::CreatedBy => "created_by",
        }
    }

    /// The field called `name`, if events may set one.
    pub fn from_name(name: &str) -> Option<Field> {
        Field::ALL.into_iter().find(|field| field.name() == name)
    }

    /// The type it holds.
    pub fn kind(self) -> FieldType {
        match self {
            Field::Priority | Field::EstimatedMinutes => FieldType::Int,
            _ => FieldType::Str,
        }
    }

    /// Its position in [`Field::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// Whether only a create may set it (`created_at`, `updated_at`, `created_by`).
    pub fn bookkeeping(self) -> bool {
        matches!(self, Field::CreatedAt | Field::UpdatedAt | Field::CreatedBy)
    }
}

/// Fields every issue shows, which cannot be cleared.
pub const REQUIRED_FIELDS: [Field; 4] = [
    Field::Title,
    Field::Priority,
    Field::IssueType,
    Field::Status,
];

/// The fields a create keeps as registers: every value a writer had not seen replaced.
pub const REGISTERS: [Field; 10] = [
    Field::Title,
    Field::Description,
    Field::Design,
    Field::AcceptanceCriteria,
    Field::Notes,
    Field::Priority,
    Field::IssueType,
    Field::Assignee,
    Field::EstimatedMinutes,
    Field::ExternalRef,
];

/// The fields of a Beads record, in the order br writes them (Python's `EXPORT_FIELDS`).
pub const EXPORT_FIELDS: [Field; 16] = [
    Field::Title,
    Field::Description,
    Field::Design,
    Field::AcceptanceCriteria,
    Field::Notes,
    Field::Status,
    Field::Priority,
    Field::IssueType,
    Field::Assignee,
    Field::EstimatedMinutes,
    Field::CreatedAt,
    Field::CreatedBy,
    Field::UpdatedAt,
    Field::ClosedAt,
    Field::CloseReason,
    Field::ExternalRef,
];

/// Python's `isinstance(value, int)`: an integer, or a boolean.
pub fn is_int(value: &Value) -> bool {
    matches!(value, Value::Int(_) | Value::Bool(_))
}

/// An integer field's number as Python compares it (`True` is 1), if it holds an integer.
fn number(value: &Value) -> Option<Int> {
    PyInt::read(value).map(|value| value.number())
}

/// Python's `isinstance(value, (str, type(None)))`, where `None` also stands for an absent key.
fn str_or_none(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null | Value::Str(_)))
}

/// Whether `value` (absent counts as `None`) is an object or `None`.
fn object_or_none(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null | Value::Object(_)))
}

/// Why an event's envelope cannot be used, or `None` (Python's `event_problem`).
pub fn event_problem(event: &Value) -> Option<String> {
    let Value::Object(event) = event else {
        return Some("event is not an object".into());
    };
    let id = event
        .get("id")
        .and_then(Value::as_str)
        .and_then(EventId::parse);
    let Some(id) = id else {
        return Some("id is not a UUID".into());
    };
    if id.millis() > MAX_MILLIS {
        return Some("id carries an impossible timestamp".into());
    }
    if !matches!(event.get("op"), Some(Value::Str(_)))
        || !matches!(event.get("replica"), Some(Value::Str(_)))
    {
        return Some("op or replica missing".into());
    }
    let deps_valid = match event.get("deps") {
        None => true,
        Some(Value::Array(deps)) => deps
            .iter()
            .all(|dep| dep.as_str().and_then(EventId::parse).is_some()),
        Some(_) => false,
    };
    if !deps_valid {
        return Some("deps is not a list of event ids".into());
    }
    if !object_or_none(event.get("data")) {
        return Some("data is not an object".into());
    }
    if !event.get("v").is_none_or(is_int) {
        return Some("v is not an integer".into());
    }
    if !str_or_none(event.get("issue")) {
        return Some("issue is not an id".into());
    }
    let actor_valid = match event.get("actor") {
        None | Some(Value::Null) => true,
        Some(Value::Object(actor)) => ["agent", "thread", "name"]
            .iter()
            .all(|key| str_or_none(actor.get(*key))),
        Some(_) => false,
    };
    if !actor_valid {
        return Some("actor is malformed".into());
    }
    let where_valid = match event.get("where") {
        None | Some(Value::Null) => true,
        Some(Value::Object(place)) => {
            str_or_none(place.get("branch")) && str_or_none(place.get("commit"))
        }
        Some(_) => false,
    };
    if !where_valid {
        return Some("where is malformed".into());
    }
    None
}

/// Why `fields` cannot be written, or `None` (Python's `fields_problem`); `bookkeeping` allows
/// `created_at`, `updated_at` and `created_by`, as a create may carry them.
pub fn fields_problem(fields: &Value, bookkeeping: bool) -> Option<String> {
    let Value::Object(fields) = fields else {
        return Some("fields is not an object".into());
    };
    for (name, value) in fields.iter() {
        let Some(field) =
            Field::from_name(name).filter(|field| bookkeeping || !field.bookkeeping())
        else {
            return Some(format!("field {} is not writable", repr_str(name)));
        };
        if *value == Value::Null {
            // Lists sort by creation time, so a create cannot clear it either (§5 row 28).
            if REQUIRED_FIELDS.contains(&field) || field == Field::CreatedAt {
                return Some(format!("field {} cannot be cleared", repr_str(name)));
            }
            continue;
        }
        let typed = match field.kind() {
            FieldType::Str => matches!(value, Value::Str(_)),
            FieldType::Int => is_int(value),
        };
        if !typed {
            return Some(format!("field {} has the wrong type", repr_str(name)));
        }
        if field == Field::Priority
            && !number(value).is_some_and(|p| Int::from(0) <= p && p <= Int::from(4))
        {
            return Some("priority is out of range".into());
        }
        if field == Field::Status
            && let Value::Str(status) = value
            && !STATUSES.contains(&status.as_str())
            && !status_shaped(status)
        {
            return Some(format!("unknown status {}", repr_str(status)));
        }
    }
    None
}

/// Whether a status other tools may write looks like one: `[a-z][a-z0-9_-]{0,39}`, whole.
pub fn status_shaped(status: &str) -> bool {
    let mut chars = status.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && status.chars().count() <= 40
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// The `data` object of a valid event: `None` and an absent one read as empty, as
/// `event.get("data") or {}` does.
pub fn data_of(event: &Value) -> Option<&Object> {
    event.get("data").and_then(Value::as_object)
}

/// Why an event's content cannot be applied, or `None` (Python's `payload_problem`). Called only on
/// an event whose envelope is valid.
pub fn payload_problem(event: &Value) -> Option<String> {
    let op = event.get("op").and_then(Value::as_str).unwrap_or_default();
    let data = data_of(event);
    let get = |key: &str| data.and_then(|d| member(d, key));
    if !KNOWN_OPS.contains(&op) {
        return Some(format!("unknown operation {}", repr_str(op)));
    }
    let strings = |key: &str| match get(key) {
        None => true,
        Some(Value::Array(items)) => items.iter().all(|item| matches!(item, Value::Str(_))),
        Some(_) => false,
    };
    match op {
        "create" => create_problem(event, data, &strings),
        "set" => match get("fields") {
            None => Some("fields is not an object".into()),
            Some(fields) => fields_problem(fields, false),
        },
        "dep_add" | "dep_remove" => {
            let has_to = data.is_some_and(|d| member(d, "to").is_some());
            let target = if has_to || op == "dep_add" {
                get("to")
            } else {
                get("key")
            };
            let valid_target = matches!(target, Some(Value::Str(t)) if !t.is_empty());
            if !valid_target || !str_or_none(get("type")) {
                return Some(format!("{op} event has no valid target"));
            }
            // What apply reads beside the target: dep_add's meta, dep_remove's key.
            let (name, valid) = if op == "dep_add" {
                ("meta", object_or_none(get("meta")))
            } else {
                ("key", str_or_none(get("key")))
            };
            (!valid).then(|| format!("{op} event has a malformed {}", repr_str(name)))
        }
        "label_add" | "label_remove" | "label_set" => {
            (!strings("labels")).then(|| format!("{op} event has malformed labels"))
        }
        "comment" => {
            let text_valid = matches!(get("text"), None | Some(Value::Str(_)));
            (!text_valid || !str_or_none(get("author")))
                .then(|| "comment event is malformed".into())
        }
        "claim" | "release" | "close" | "reopen" | "delete" => {
            for key in ["mode", "takeover", "reason", "claim"] {
                if !str_or_none(get(key)) {
                    return Some(format!("{op} event has a malformed {}", repr_str(key)));
                }
            }
            (!get("generation").is_none_or(is_int))
                .then(|| "claim event has a malformed generation".into())
        }
        _ => None,
    }
}

fn create_problem(
    event: &Value,
    data: Option<&Object>,
    strings: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let get = |key: &str| data.and_then(|d| member(d, key));
    if event.get("issue") != event.get("id") {
        return Some("create event does not name itself as the issue".into());
    }
    match get("key") {
        Some(Value::Str(key)) if !key.is_empty() && !number_too_long(key) => {}
        _ => return Some("create event has no valid 'key'".into()),
    }
    if let Some(fields) = get("fields")
        && let Some(problem) = fields_problem(fields, true)
    {
        return Some(problem);
    }
    let extra_valid = matches!(get("extra"), None | Some(Value::Object(_)));
    let comments_valid = matches!(get("comments"), None | Some(Value::Array(_)));
    if !strings("labels") || !extra_valid || !comments_valid {
        return Some("create event has malformed labels, extra or comments".into());
    }
    let dangling_valid = match get("dangling") {
        None => true,
        Some(Value::Array(items)) => items.iter().all(|item| {
            let Value::Object(dep) = item else {
                return false;
            };
            matches!(dep.get("depends_on_id"), Some(Value::Str(target)) if !target.is_empty())
                && str_or_none(dep.get("type"))
        }),
        Some(_) => false,
    };
    if !dangling_valid {
        return Some("create event has malformed dangling dependencies".into());
    }
    for comment in get("comments")
        .and_then(Value::as_array)
        .unwrap_or_default()
    {
        let Value::Object(comment) = comment else {
            return Some("create event has a malformed comment".into());
        };
        let text_valid = matches!(comment.get("text"), Some(Value::Str(_)));
        let present = ["author", "created_at"].iter().all(|key| {
            comment
                .get(*key)
                .is_some_and(|v| matches!(v, Value::Null | Value::Str(_)))
        });
        let legacy_valid = matches!(
            comment.get("legacy_id"),
            None | Some(Value::Null | Value::Str(_) | Value::Int(_))
        );
        // A comment made here has a uid and a where; an imported one has neither (§5 row 27).
        let imported = !comment.contains_key("uid") && !comment.contains_key("where");
        if !text_valid || !present || !legacy_valid || !imported {
            return Some("create event has a malformed comment".into());
        }
    }
    None
}

/// Why this bead must not write an event, beyond what replay accepts (Python's `write_problem`):
/// Beads readers take priority and estimated_minutes as non-negative 32-bit integers, and a boolean
/// is not one.
pub fn write_problem(event: &Value) -> Option<String> {
    let op = event.get("op").and_then(Value::as_str).unwrap_or_default();
    if !matches!(op, "create" | "set") {
        return None;
    }
    let fields = data_of(event)
        .and_then(|d| d.get("fields"))
        .and_then(Value::as_object)?;
    for name in ["priority", "estimated_minutes"] {
        let value = fields.get(name);
        if let Some(Value::Bool(_)) = value {
            return Some(format!("field {} has the wrong type", repr_str(name)));
        }
        if name == "estimated_minutes"
            && let Some(value) = value.filter(|v| **v != Value::Null)
            && !number(value).is_some_and(|m| Int::from(0) <= m && m <= Int::from(2_147_483_647))
        {
            return Some("estimated_minutes must be between 0 and 2147483647".into());
        }
    }
    None
}
