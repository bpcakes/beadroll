//! The metadata history displays, without operation payloads or replay authority.

use crate::id::EventId;
use crate::journal::Event;
use crate::json::Value;
use crate::op::Op;

/// One settled event's display metadata. Issue ids and machine ids remain unresolved so a
/// reader uses today's keys and aliases. This is not an event that replay can apply.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    /// The event's id, including its timestamp.
    pub id: EventId,
    /// The machine id as recorded.
    pub machine: String,
    /// The actor's agent, as recorded, or null.
    pub agent: Value,
    /// The actor's thread, as recorded, or null.
    pub thread: Value,
    /// The actor's explicit name, as recorded, or null.
    pub name: Value,
    /// The operation's name, including invalid and quarantined placeholders.
    pub operation: String,
    /// The issue's internal id, when the event names one.
    pub issue: Option<String>,
    /// The branch as recorded, or null.
    pub branch: Value,
    /// Fields a `set` changed, in their original order.
    pub changed_fields: Vec<String>,
}

impl From<&Event> for HistoryEntry {
    fn from(event: &Event) -> Self {
        let actor = |key: &str| {
            event
                .actor()
                .filter(|a| a.truthy())
                .and_then(|a| a.get(key))
                .cloned()
                .unwrap_or(Value::Null)
        };
        Self {
            id: event.id(),
            machine: event.replica().to_owned(),
            agent: actor("agent"),
            thread: actor("thread"),
            name: actor("name"),
            operation: event.op_name().to_owned(),
            issue: event.issue().map(str::to_owned),
            branch: event
                .place()
                .filter(|p| p.truthy())
                .and_then(|p| p.get("branch"))
                .cloned()
                .unwrap_or(Value::Null),
            changed_fields: match event.op() {
                Op::Set(fields) => fields
                    .iter()
                    .map(|(field, _)| field.name().to_owned())
                    .collect(),
                _ => Vec::new(),
            },
        }
    }
}

impl HistoryEntry {
    /// A compact cache record. This is not part of the published journal format.
    pub fn cached(&self) -> Value {
        Value::Array(vec![
            Value::from(self.id.to_string()),
            Value::from(self.machine.as_str()),
            self.agent.clone(),
            self.thread.clone(),
            self.name.clone(),
            Value::from(self.operation.as_str()),
            Value::from(self.issue.as_deref()),
            self.branch.clone(),
            Value::Array(
                self.changed_fields
                    .iter()
                    .map(|field| Value::from(field.as_str()))
                    .collect(),
            ),
        ])
    }

    /// Reads a record written by [`Self::cached`], or refuses a malformed cache.
    pub fn from_cached(value: &Value) -> Option<Self> {
        let [
            id,
            machine,
            agent,
            thread,
            name,
            operation,
            issue,
            branch,
            fields,
        ] = value.as_array()?
        else {
            return None;
        };
        let issue = match issue {
            Value::Null => None,
            Value::Str(text) => Some(text.clone()),
            _ => return None,
        };
        Some(Self {
            id: EventId::parse(id.as_str()?)?,
            machine: machine.as_str()?.to_owned(),
            agent: agent.clone(),
            thread: thread.clone(),
            name: name.clone(),
            operation: operation.as_str()?.to_owned(),
            issue,
            branch: branch.clone(),
            changed_fields: fields
                .as_array()?
                .iter()
                .map(|field| field.as_str().map(str::to_owned))
                .collect::<Option<_>>()?,
        })
    }
}
