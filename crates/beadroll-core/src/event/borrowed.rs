//! The envelope rules over borrowed members, shared by typed lines and values built in memory.

use crate::id::EventId;
use crate::json::{Object, Value};
use crate::time::MAX_MILLIS;

/// Only what validation inspects. Unrelated values never need cloning or ownership here.
pub(super) struct Borrowed<'a> {
    pub(super) object: bool,
    pub(super) id: Option<EventId>,
    pub(super) op: Option<&'a str>,
    pub(super) replica: Option<&'a str>,
    pub(super) deps_valid: bool,
    pub(super) data_valid: bool,
    pub(super) data: Option<&'a Object>,
    pub(super) v: Option<&'a Value>,
    pub(super) issue_valid: bool,
    pub(super) issue: Option<&'a str>,
    pub(super) actor: Option<&'a Value>,
    pub(super) place: Option<&'a Value>,
}

impl<'a> Borrowed<'a> {
    /// Views the original members, without walking a payload or an unknown member.
    pub(super) fn of(event: &'a Value) -> Self {
        Self {
            object: matches!(event, Value::Object(_)),
            id: event
                .get("id")
                .and_then(Value::as_str)
                .and_then(EventId::parse),
            op: event.get("op").and_then(Value::as_str),
            replica: event.get("replica").and_then(Value::as_str),
            deps_valid: match event.get("deps") {
                None => true,
                Some(Value::Array(items)) => items
                    .iter()
                    .all(|item| item.as_str().and_then(EventId::parse).is_some()),
                Some(_) => false,
            },
            data_valid: super::object_or_none(event.get("data")),
            data: super::data_of(event),
            v: event.get("v"),
            issue_valid: super::str_or_none(event.get("issue")),
            issue: event.get("issue").and_then(Value::as_str),
            actor: event.get("actor"),
            place: event.get("where"),
        }
    }

    /// The envelope's first problem, in the established diagnostic order.
    pub(super) fn problem(&self) -> Option<String> {
        if !self.object {
            return Some("event is not an object".into());
        }
        let Some(id) = self.id else {
            return Some("id is not a UUID".into());
        };
        if id.millis() > MAX_MILLIS {
            return Some("id carries an impossible timestamp".into());
        }
        if self.op.is_none() || self.replica.is_none() {
            return Some("op or replica missing".into());
        }
        if !self.deps_valid {
            return Some("deps is not a list of event ids".into());
        }
        if !self.data_valid {
            return Some("data is not an object".into());
        }
        if !self.v.is_none_or(super::is_int) {
            return Some("v is not an integer".into());
        }
        if !self.issue_valid {
            return Some("issue is not an id".into());
        }
        let actor_valid = match self.actor {
            None | Some(Value::Null) => true,
            Some(Value::Object(actor)) => ["agent", "thread", "name"]
                .iter()
                .all(|key| super::str_or_none(actor.get(*key))),
            Some(_) => false,
        };
        if !actor_valid {
            return Some("actor is malformed".into());
        }
        let where_valid = match self.place {
            None | Some(Value::Null) => true,
            Some(Value::Object(place)) => {
                super::str_or_none(place.get("branch")) && super::str_or_none(place.get("commit"))
            }
            Some(_) => false,
        };
        if !where_valid {
            return Some("where is malformed".into());
        }
        None
    }

    /// The payload rules inspect the same borrowed fields for either input route.
    pub(super) fn payload_problem(&self) -> Option<String> {
        super::payload_problem_of(self.op.unwrap_or_default(), self.data, self.id, self.issue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{event_problem, payload_problem};
    use crate::json::parse;

    fn nested() -> Value {
        (0..100_000).fold(Value::Null, |inner, _| Value::Array(vec![inner]))
    }

    fn dispose(mut value: Value) {
        while let Value::Array(mut items) = value {
            value = items.pop().unwrap_or(Value::Null);
        }
    }

    #[test]
    fn deeply_nested_non_object_is_reported_without_copying_it() {
        let value = nested();
        let problem = event_problem(&value);
        dispose(value);
        assert_eq!(problem.as_deref(), Some("event is not an object"));
    }

    #[test]
    fn validation_borrows_past_deep_unused_members() {
        let Value::Object(mut event) = parse(br#"{"id":"00000000-0000-7000-8000-000000000001","op":"set","replica":"r","data":{"fields":{}}}"#).unwrap().value else {
            unreachable!()
        };
        event.insert("unused".into(), nested());
        let value = Value::Object(event);
        let envelope = event_problem(&value);
        let payload = payload_problem(&value);
        let Value::Object(mut event) = value else {
            unreachable!()
        };
        dispose(event.shift_remove("unused").unwrap());
        assert_eq!(envelope, None);
        assert_eq!(payload, None);
    }
}
