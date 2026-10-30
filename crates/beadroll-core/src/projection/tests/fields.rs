//! Field values drawn by the projection and replay differential histories.

use crate::json::{Object, Value};

use super::Rng;

/// Whether a generated field value may be null.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldValue {
    /// Required.
    Required,
    /// Nullable.
    Nullable,
}

/// Whether a generated create imports an existing issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CreateOrigin {
    /// Local.
    Local,
    /// Imported.
    Imported,
}

/// The fields of a generated create, which now and then leaves a field to its default, gives one
/// null or a legacy boolean priority, or makes an issue closed with what it gives of the close;
/// an import's, now and then the times and the author its record gives, the author null included.
pub(super) fn created(r: &mut Rng, roll: usize, imported: CreateOrigin) -> Object {
    let imported = matches!(imported, CreateOrigin::Imported);
    let mut fields = Object::new();
    for field in ["title", "priority", "issue_type"] {
        if !r.chance(10) {
            fields.insert(field.into(), drawn(r, field, roll, FieldValue::Required));
        }
    }
    match r.below(10) {
        0 => {}
        1 => {
            fields.insert("status".into(), Value::from("closed"));
            for field in ["closed_at", "close_reason"] {
                let value = match r.below(3) {
                    0 => continue,
                    1 => Value::Null,
                    _ => drawn(r, field, roll, FieldValue::Required),
                };
                fields.insert(field.into(), value);
            }
        }
        _ => {
            fields.insert("status".into(), Value::from("open"));
        }
    }
    for field in ["assignee", "estimated_minutes", "external_ref"] {
        if r.chance(40) {
            fields.insert(field.into(), drawn(r, field, roll, FieldValue::Nullable));
        }
    }
    for field in ["created_at", "updated_at", "created_by"] {
        if imported && r.chance(50) {
            let value = match field {
                "created_by" if r.chance(30) => Value::Null,
                "created_by" => Value::from(format!("v{roll}")),
                _ => Value::from(format!("2025-12-{:02}T00:00:00Z", 1 + r.below(28))),
            };
            fields.insert(field.into(), value);
        }
    }
    fields
}

/// The fields of a generated set: one to three, in the order drawn, so that a status comes before
/// or after a closed_at or close_reason, which a set writes into registers of their own.
pub(super) fn written(r: &mut Rng, roll: usize) -> Object {
    let mut fields = Object::new();
    for _ in 0..=r.below(3) {
        let field = *r.pick(&SET).expect("fields");
        if !fields.contains_key(field) {
            let value = drawn(r, field, roll, FieldValue::Nullable);
            fields.insert(field.into(), value);
        }
    }
    fields
}

/// The fields a generated set writes.
const SET: [&str; 10] = [
    "title",
    "assignee",
    "description",
    "priority",
    "estimated_minutes",
    "external_ref",
    "status",
    "closed_at",
    "close_reason",
    "notes",
];

/// A value of `field` for an event of `roll` to write: null now and then where `nullable` and the
/// field may be cleared, and a legacy boolean priority now and then.
fn drawn(r: &mut Rng, field: &str, roll: usize, nullable: FieldValue) -> Value {
    let nullable = matches!(nullable, FieldValue::Nullable);
    let required = matches!(field, "title" | "priority" | "issue_type" | "status");
    if nullable && !required && r.chance(30) {
        return Value::Null;
    }
    match field {
        "priority" if r.chance(15) => Value::Bool(r.chance(50)),
        "priority" => Value::from(r.below(5) as i64),
        "estimated_minutes" => Value::from(roll as i64),
        "issue_type" => Value::from(["task", "bug"][r.below(2)]),
        "status" => Value::from(["open", "in_progress", "closed"][r.below(3)]),
        "closed_at" => Value::from(format!("2026-01-{:02}T00:00:00Z", 1 + roll % 28)),
        _ => Value::from(format!("v{roll}")),
    }
}
