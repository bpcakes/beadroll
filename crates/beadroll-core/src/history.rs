//! The metadata history displays, without operation payloads or replay authority.

use crate::id::EventId;
use crate::journal::Event;
use crate::json::Value;
use crate::op::Op;
use crate::projection::Unreadable;
use crate::projection::codec::{Reader, Writer};

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
    /// The claim a `claim` took over, by its event id, when it is a takeover.
    pub takeover_of: Option<String>,
}

impl From<&Event> for HistoryEntry {
    fn from(event: &Event) -> Self {
        let actor = |key: &str| {
            event
                .actor()
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
            takeover_of: match event.op() {
                Op::Claim(claim) => claim.takeover_of().map(str::to_owned),
                _ => None,
            },
        }
    }
}

/// The bytes a cache keeps of `entries`, in order: no part of format 1, written in the codec a
/// projection is ([`crate::projection`]), and read back only by the build that wrote them
/// ([`from_cached`]).
pub fn cached(entries: &[HistoryEntry]) -> Vec<u8> {
    let mut w = Writer::default();
    w.count(entries.len());
    for entry in entries {
        entry.write(&mut w);
    }
    w.finish()
}

/// The fewest bytes an entry is written in: its id's 16, and one for each of its other nine fields.
const ENTRY_BYTES: usize = 16 + 9;

/// The entries of bytes [`cached`] wrote; `None` for any other bytes, which a cache that is
/// damaged holds. Room is made for no more entries than the bytes can hold, whatever count they
/// claim.
pub fn from_cached(bytes: &[u8]) -> Option<Vec<HistoryEntry>> {
    read_cache(bytes).ok()
}

fn read_cache(bytes: &[u8]) -> Result<Vec<HistoryEntry>, Unreadable> {
    let mut r = Reader::new(bytes);
    let count = r.count()?;
    if count > (bytes.len() - r.at()) / ENTRY_BYTES {
        return Err(Unreadable("a history entry count"));
    }
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(count)
        .map_err(|_| Unreadable("history entry allocation"))?;
    for _ in 0..count {
        entries.push(HistoryEntry::read(&mut r)?);
    }
    r.done()?;
    Ok(entries)
}

impl HistoryEntry {
    fn write(&self, w: &mut Writer) {
        w.id(self.id);
        w.text(&self.machine);
        w.value(&self.agent);
        w.value(&self.thread);
        w.value(&self.name);
        w.text(&self.operation);
        w.optional_text(self.issue.as_deref());
        w.value(&self.branch);
        w.count(self.changed_fields.len());
        for field in &self.changed_fields {
            w.text(field);
        }
        w.optional_text(self.takeover_of.as_deref());
    }

    fn read(r: &mut Reader<'_>) -> Result<HistoryEntry, Unreadable> {
        Ok(HistoryEntry {
            id: r.id()?,
            machine: r.text()?,
            agent: r.value()?,
            thread: r.value()?,
            name: r.value()?,
            operation: r.text()?,
            issue: r.optional_text()?,
            branch: r.value()?,
            // grown as they are read: a field takes a byte at least, and its text 24 in memory
            changed_fields: (0..r.count()?)
                .map(|_| r.text())
                .collect::<Result<_, _>>()?,
            takeover_of: r.optional_text()?,
        })
    }
}

#[cfg(test)]
mod tests;
