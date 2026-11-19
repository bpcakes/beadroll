//! Event validation: the shared envelope (`event_problem`), each operation's payload
//! (`payload_problem`) and stricter rules for new writes (`write_problem`). Each returns a
//! diagnostic or `None`. The envelope's rules are judged on what was read of it ([`Envelope`],
//! roll-lqjj.53.1), which a journal line is read straight into and an event built in memory walks
//! into.

mod borrowed;
mod envelope;

use crate::id::EventId;
use crate::json::{Int, Object, Value, member};
use crate::key::number_too_long;
use crate::op::{IntegerField, OpName};
use crate::text::repr_str;

pub(crate) use envelope::{Data, Envelope};

/// Which fields an operation may write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldScope {
    /// A `set`: only the fields a user edits.
    Editable,
    /// A `create`: `created_at`, `updated_at` and `created_by` too.
    Create,
}

/// The format of events this bead reads and writes.
pub const FORMAT: i64 = 1;

/// The statuses bead's own commands set, one name each. Beads tools write others, which bead keeps
/// as they are, so a status stays text.
pub mod status {
    /// Waiting to be worked on.
    pub const OPEN: &str = "open";
    /// Being worked on: what `update --status in_progress` asks of a claim.
    pub const IN_PROGRESS: &str = "in_progress";
    /// Waiting on something outside the tracker.
    pub const BLOCKED: &str = "blocked";
    /// Put off; left out of a listing unless asked for.
    pub const DEFERRED: &str = "deferred";
    /// Done.
    pub const CLOSED: &str = "closed";
    /// Deleted: kept so that what named it still resolves.
    pub const TOMBSTONE: &str = "tombstone";
}

/// The statuses bead's own commands set; Beads tools write others, kept as they are.
pub const STATUSES: [&str; 6] = [
    status::OPEN,
    status::IN_PROGRESS,
    status::BLOCKED,
    status::DEFERRED,
    status::CLOSED,
    status::TOMBSTONE,
];

/// Statuses that count as finished.
pub const DONE: [&str; 2] = [status::CLOSED, status::TOMBSTONE];

/// The dependency type that keeps its issue waiting until the target is done, and the type of a
/// dependency that names none.
pub const BLOCKS: &str = "blocks";

/// The dependency type that makes its target the issue's parent.
pub const PARENT_CHILD: &str = "parent-child";

/// The maximum nesting of arrays and objects in an event this bead writes.
pub const MAX_NESTING: usize = 100;

/// The type an issue field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    /// A string.
    Str,
    /// An integer, including legacy boolean values during replay.
    Int,
}

/// An editable issue field, in validation and display order.
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
    /// Every field, in the order they are declared.
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

/// The fields of a Beads record, in the order br writes them.
pub(crate) const EXPORT_FIELDS: [Field; 16] = [
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

/// Whether the value is an integer or a legacy boolean integer field.
pub(crate) fn is_int(value: &Value) -> bool {
    matches!(value, Value::Int(_) | Value::Bool(_))
}

/// An integer field's numeric value, mapping true to 1 and false to 0.
fn number(value: &Value) -> Option<Int> {
    IntegerField::read(value).map(|value| value.number())
}

/// Whether the member is absent, null or a string.
fn str_or_none(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null | Value::Str(_)))
}

/// Whether `value` is absent, null or an object.
fn object_or_none(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null | Value::Object(_)))
}

/// Why an event's envelope cannot be used, or `None`: [`Envelope::problem`] of what is read of
/// `event`.
pub fn event_problem(event: &Value) -> Option<String> {
    borrowed::Borrowed::of(event).problem()
}

/// Why `fields` cannot be written, or `None`. A create's fields (`FieldScope::Create`) may hold
/// `created_at`, `updated_at` and `created_by` too.
pub(crate) fn fields_problem(fields: &Value, scope: FieldScope) -> Option<String> {
    let Value::Object(fields) = fields else {
        return Some("fields is not an object".into());
    };
    for (name, value) in fields.iter() {
        let Some(field) = Field::from_name(name)
            .filter(|field| scope == FieldScope::Create || !field.bookkeeping())
        else {
            return Some(format!("field {} is not writable", repr_str(name)));
        };
        if *value == Value::Null {
            // Lists sort by creation time, so a create cannot clear it either (U09).
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
pub(crate) fn status_shaped(status: &str) -> bool {
    let mut chars = status.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && status.chars().count() <= 40
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// The `data` object of a valid event; null or absent data yields `None` and is treated as empty.
pub(crate) fn data_of(event: &Value) -> Option<&Object> {
    event.get("data").and_then(Value::as_object)
}

/// Why an event's content cannot be applied, or `None`. Called only after envelope validation.
pub fn payload_problem(event: &Value) -> Option<String> {
    borrowed::Borrowed::of(event).payload_problem()
}

/// Why the payload `data` of the operation `op` cannot be applied, or `None`, for the event `id`
/// about `issue` (a create must name itself). Called only after envelope validation.
pub(crate) fn payload_problem_of(
    op: &str,
    data: Option<&Object>,
    id: Option<EventId>,
    issue: Option<&str>,
) -> Option<String> {
    let get = |key: &str| data.and_then(|d| member(d, key));
    let Some(name) = OpName::from_name(op) else {
        return Some(format!("unknown operation {}", repr_str(op)));
    };
    let strings = |key: &str| match get(key) {
        None => true,
        Some(Value::Array(items)) => items.iter().all(|item| matches!(item, Value::Str(_))),
        Some(_) => false,
    };
    match name {
        OpName::Create => {
            // An id's spelling is exact, so the issue names it exactly when it spells it.
            let names_itself = id.is_some() && issue.and_then(EventId::parse) == id;
            if !names_itself {
                Some("create event does not name itself as the issue".into())
            } else {
                create_problem(data, &strings)
            }
        }
        OpName::Set => match get("fields") {
            None => Some("fields is not an object".into()),
            Some(fields) => fields_problem(fields, FieldScope::Editable),
        },
        OpName::DepAdd | OpName::DepRemove => {
            let has_to = data.is_some_and(|d| member(d, "to").is_some());
            let target = if has_to || name == OpName::DepAdd {
                get("to")
            } else {
                get("key")
            };
            let valid_target = matches!(target, Some(Value::Str(t)) if !t.is_empty());
            if !valid_target || !str_or_none(get("type")) {
                return Some(format!("{op} event has no valid target"));
            }
            // What apply reads beside the target: dep_add's meta, dep_remove's key.
            let (member_name, valid) = if name == OpName::DepAdd {
                ("meta", object_or_none(get("meta")))
            } else {
                ("key", str_or_none(get("key")))
            };
            (!valid).then(|| format!("{op} event has a malformed {}", repr_str(member_name)))
        }
        OpName::LabelAdd | OpName::LabelRemove | OpName::LabelSet => {
            (!strings("labels")).then(|| format!("{op} event has malformed labels"))
        }
        OpName::Comment => {
            let text_valid = matches!(get("text"), None | Some(Value::Str(_)));
            (!text_valid || !str_or_none(get("author")))
                .then(|| "comment event is malformed".into())
        }
        OpName::Claim | OpName::Release | OpName::Close | OpName::Reopen | OpName::Delete => {
            for key in ["mode", "takeover", "reason", "claim"] {
                if !str_or_none(get(key)) {
                    return Some(format!("{op} event has a malformed {}", repr_str(key)));
                }
            }
            (!get("generation").is_none_or(is_int))
                .then(|| "claim event has a malformed generation".into())
        }
        OpName::Init => None,
    }
}

fn create_problem(data: Option<&Object>, strings: &dyn Fn(&str) -> bool) -> Option<String> {
    let get = |key: &str| data.and_then(|d| member(d, key));
    match get("key") {
        Some(Value::Str(key)) if !key.is_empty() && !number_too_long(key) => {}
        _ => return Some("create event has no valid 'key'".into()),
    }
    if let Some(fields) = get("fields")
        && let Some(problem) = fields_problem(fields, FieldScope::Create)
    {
        return Some(problem);
    }
    // added to format 1 (#307): the parent a child was made under, by its id
    let parent_valid = match get("parent") {
        None => true,
        Some(Value::Str(parent)) => crate::id::EventId::parse(parent).is_some(),
        Some(_) => false,
    };
    if !parent_valid {
        return Some("create event has a malformed 'parent'".into());
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
        // A comment made here has a uid and a where; an imported one has neither (U08).
        let imported = !comment.contains_key("uid") && !comment.contains_key("where");
        if !text_valid || !present || !legacy_valid || !imported {
            return Some("create event has a malformed comment".into());
        }
    }
    None
}

/// Stricter validation for new writes: priority and estimated_minutes must be non-negative 32-bit
/// integers for Beads readers; booleans are refused even though legacy replay accepts them.
pub fn write_problem(event: &Value) -> Option<String> {
    let op = event
        .get("op")
        .and_then(Value::as_str)
        .and_then(OpName::from_name);
    if !matches!(op, Some(OpName::Create | OpName::Set)) {
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
