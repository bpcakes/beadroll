//! Journals as bead reads them (Python's `read_segment`, `read_attempt`, `loads_line` and `Journal`):
//! lines parsed into events, and the events of every file settled together, so that a duplicated or
//! malformed record is judged the same way whatever order the files were read in.

use std::collections::HashSet;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::event::{event_problem, payload_problem};
use crate::id::EventId;
use crate::json::{self, Format, Object, ParseError, Value};
use crate::text::strip_bytes;

/// Something a reader could not use: an unreadable line or file, an event that fails validation, a
/// copy in the wrong journal. Replay reports each as an integrity row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The event's id, when one could be read.
    pub event: Option<String>,
    /// Where it was found: a path, with `:LINE` for a line.
    pub origin: String,
    /// What is wrong, in Python's words.
    pub problem: String,
}

impl Problem {
    fn new(event: Option<String>, origin: String, problem: impl Into<String>) -> Problem {
        Problem {
            event,
            origin,
            problem: problem.into(),
        }
    }
}

/// One journal or attempt line (Python's `loads_line`): the value, with a lone surrogate read as
/// U+FFFD and reported, because such text can be neither printed nor exported.
pub fn loads_line(
    line: &[u8],
    origin: &str,
    number: usize,
    problems: Option<&mut Vec<Problem>>,
) -> Result<Value, ParseError> {
    let parsed = json::parse(line)?;
    if parsed.lone_surrogates
        && let Some(problems) = problems
    {
        let event = parsed
            .value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let origin = format!("{origin}:{}", number + 1);
        problems.push(Problem::new(
            event,
            origin,
            "text is not valid UTF-8; it reads as U+FFFD",
        ));
    }
    Ok(parsed.value)
}

/// The events of one segment (Python's `read_segment`). A torn tail (an unreadable last line, which
/// another process may still be appending) is skipped; any other unreadable line is reported.
pub fn read_segment(
    data: &[u8],
    origin: &str,
    mut problems: Option<&mut Vec<Problem>>,
) -> Vec<Value> {
    let lines: Vec<&[u8]> = data.split(|b| *b == b'\n').collect();
    let mut events = Vec::new();
    for (number, line) in lines.iter().enumerate() {
        if strip_bytes(line).is_empty() {
            continue;
        }
        match loads_line(line, origin, number, problems.as_deref_mut()) {
            Ok(event) => events.push(event),
            Err(_) if number + 1 == lines.len() => break,
            Err(_) => {
                if let Some(problems) = problems.as_deref_mut() {
                    let origin = format!("{origin}:{}", number + 1);
                    problems.push(Problem::new(None, origin, "line is not valid JSON"));
                }
            }
        }
    }
    events
}

/// The events of one attempt file (Python's `read_attempt` on bytes already read): an attempt counts
/// whole or not at all, so any bad line empties it, reported.
pub fn read_attempt(
    data: &[u8],
    origin: &str,
    mut problems: Option<&mut Vec<Problem>>,
) -> Vec<Value> {
    let mut events = Vec::new();
    for (number, line) in data.split(|b| *b == b'\n').enumerate() {
        if strip_bytes(line).is_empty() {
            continue;
        }
        let problem = match loads_line(line, origin, number, problems.as_deref_mut()) {
            Err(_) => Some("line is not valid JSON".to_owned()),
            Ok(event) if !matches!(event, Value::Object(_)) => {
                Some("attempt line is not an event".to_owned())
            }
            Ok(event) => match event_problem(&event).or_else(|| payload_problem(&event)) {
                None => {
                    events.push(event);
                    None
                }
                problem => problem,
            },
        };
        if let Some(problem) = problem {
            if let Some(problems) = problems.as_deref_mut() {
                let origin = format!("{origin}:{}", number + 1);
                problems.push(Problem::new(
                    None,
                    origin,
                    format!("{problem}; the whole attempt is ignored"),
                ));
            }
            return Vec::new();
        }
    }
    events
}

/// An event as replay sees it: a valid event, or the stand-in for an id that cannot be used
/// (`op` `quarantined` or `invalid`). Its object is kept whole, shared with the record it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    id: EventId,
    value: Arc<Value>,
    deps: Vec<EventId>,
}

impl Event {
    fn new(id: EventId, value: Arc<Value>) -> Event {
        let deps = value
            .get("deps")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|dep| dep.as_str().and_then(EventId::parse))
            .collect();
        Event { id, value, deps }
    }

    /// Its id.
    pub fn id(&self) -> EventId {
        self.id
    }

    /// The whole event.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// The events it names as causes, as listed (`deps`).
    pub fn deps(&self) -> &[EventId] {
        &self.deps
    }

    /// Its operation.
    pub fn op(&self) -> &str {
        self.value
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The machine that wrote it.
    pub fn replica(&self) -> &str {
        self.value
            .get("replica")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The issue it is about, if it names one.
    pub fn issue(&self) -> Option<&str> {
        self.value.get("issue").and_then(Value::as_str)
    }

    /// Its member `key`: Python's `event.get(key)`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.value.get(key)
    }

    /// Its payload, empty when it has none: Python's `event.get("data") or {}`.
    pub fn data(&self) -> &Object {
        static EMPTY: std::sync::LazyLock<Object> = std::sync::LazyLock::new(Object::new);
        self.value
            .get("data")
            .and_then(Value::as_object)
            .unwrap_or(&EMPTY)
    }
}

/// One record of an id as it was added: the event, the journal it was found in, where, whether it
/// has a place in that journal's order, and when it was added.
#[derive(Clone, Debug)]
struct Record {
    event: Arc<Value>,
    replica: String,
    origin: String,
    sequenced: bool,
    counter: u64,
}

impl Record {
    /// Whether it is a valid event found in its own machine's journal.
    fn own(&self) -> bool {
        event_problem(&self.event).is_none()
            && self.event.get("replica").and_then(Value::as_str) == Some(&self.replica)
    }
}

/// Every event a machine holds, with each machine's own append order (Python's `Journal`). Raw
/// records are collected first and settled together.
#[derive(Clone, Debug)]
pub struct Journal {
    events: IndexMap<EventId, Event>,
    order: IndexMap<String, Vec<EventId>>,
    quarantined: IndexMap<EventId, Vec<Value>>,
    /// What could not be used, in the order found; replay reports each.
    pub invalid: Vec<Problem>,
    raw: IndexMap<EventId, Vec<Record>>,
    counter: u64,
    settled: bool,
    referenced: HashSet<EventId>,
}

impl Default for Journal {
    fn default() -> Journal {
        Journal::new()
    }
}

impl Journal {
    /// An empty journal.
    pub fn new() -> Journal {
        Journal {
            events: IndexMap::new(),
            order: IndexMap::new(),
            quarantined: IndexMap::new(),
            invalid: Vec::new(),
            raw: IndexMap::new(),
            counter: 0,
            settled: true,
            referenced: HashSet::new(),
        }
    }

    /// Adds a record found in `replica`'s journal at `origin`; `sequenced` is false for an attempt
    /// file's events, which have no place in the journal's order. Anything without a readable id is
    /// reported at once; the rest is judged when the journal settles.
    pub fn add(&mut self, event: Value, replica: &str, origin: &str, sequenced: bool) {
        let id = event
            .get("id")
            .and_then(Value::as_str)
            .and_then(EventId::parse);
        let Some(id) = id else {
            let problem = event_problem(&event).unwrap_or_else(|| "id is not a UUID".to_owned());
            self.invalid
                .push(Problem::new(None, origin.to_owned(), problem));
            return;
        };
        self.counter += 1;
        let event = Arc::new(event);
        let record = Record {
            event: Arc::clone(&event),
            replica: replica.to_owned(),
            origin: origin.to_owned(),
            sequenced,
            counter: self.counter,
        };
        let records = self.raw.entry(id).or_default();
        let fresh = records.is_empty() && record.own();
        records.push(record);
        if self.settled && !self.events.contains_key(&id) && fresh {
            self.install(id, event, replica, sequenced); // the common case: a fresh, valid event
        } else {
            self.settled = false;
        }
    }

    fn install(&mut self, id: EventId, event: Arc<Value>, replica: &str, sequenced: bool) {
        let event = Event::new(id, event);
        if sequenced {
            self.order.entry(replica.to_owned()).or_default().push(id);
        }
        self.referenced.extend(event.deps.iter().copied());
        self.events.insert(id, event);
    }

    /// Rebuilds from the raw records: one content per id is an event; several contents are a
    /// quarantined id; a malformed record keeps its place as an inert placeholder; a copy found only
    /// in another machine's journal is ignored.
    pub fn settle(&mut self) {
        if self.settled {
            return;
        }
        self.events.clear();
        self.order.clear();
        self.quarantined.clear();
        self.referenced.clear();
        self.invalid.retain(|entry| entry.event.is_none()); // unreadable lines and files stay reported
        let raw = std::mem::take(&mut self.raw);
        let mut placed: Vec<(EventId, &Vec<Record>, bool, &Record)> = Vec::new();
        for (id, records) in &raw {
            let own = records.iter().find(|record| record.own());
            let placement = own.or_else(|| records.first());
            if let Some(placement) = placement {
                placed.push((*id, records, own.is_some(), placement));
            }
        }
        placed.sort_by_key(|(_, _, _, placement)| placement.counter); // each journal keeps its own order
        for (id, records, own, placement) in placed {
            // Each content once, in the order found: what Python's `canonical` tells apart.
            let mut distinct: Vec<&Arc<Value>> = Vec::new();
            for record in records {
                if !distinct
                    .iter()
                    .any(|seen| same_content(seen, &record.event))
                {
                    distinct.push(&record.event);
                }
            }
            let (event, replica) = (&placement.event, placement.replica.as_str());
            let (origin, sequenced) = (&placement.origin, placement.sequenced);
            if distinct.len() > 1 {
                self.quarantined
                    .insert(id, distinct.iter().map(|v| Value::clone(v)).collect());
                let trusted = if own { listed_ids(event) } else { Vec::new() };
                // A copy that is not valid may name anything: only a string is an issue (§5 row 29).
                let issue = event
                    .get("issue")
                    .and_then(Value::as_str)
                    .map_or(Value::Null, Value::from);
                let stand_in = placeholder(id, "quarantined", replica, trusted, ("issue", issue));
                self.install(id, Arc::new(stand_in), replica, sequenced);
                continue;
            }
            if let Some(problem) = event_problem(event) {
                self.invalid.push(Problem::new(
                    Some(id.to_string()),
                    origin.clone(),
                    problem.clone(),
                ));
                let stand_in = placeholder(
                    id,
                    "invalid",
                    replica,
                    listed_ids(event),
                    ("problem", problem.into()),
                );
                self.install(id, Arc::new(stand_in), replica, sequenced);
                continue;
            }
            if !own {
                let writer = event
                    .get("replica")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let problem = format!(
                    "written by machine {writer} but found only in machine {replica}'s journal"
                );
                self.invalid
                    .push(Problem::new(Some(id.to_string()), origin.clone(), problem));
                continue;
            }
            self.install(id, Arc::clone(event), replica, sequenced);
        }
        self.raw = raw;
        self.settled = true;
    }

    /// The frontier: events nothing else here has built on yet, in order.
    pub fn heads(&mut self) -> Vec<EventId> {
        self.settle();
        self.frontier()
    }

    /// The frontier of a settled journal (what [`Journal::heads`] answers once it has settled).
    pub fn frontier(&self) -> Vec<EventId> {
        let mut heads: Vec<EventId> = self
            .events
            .keys()
            .filter(|id| !self.referenced.contains(id))
            .copied()
            .collect();
        heads.sort_unstable();
        heads
    }

    /// The events, in the order they were installed. Settle first.
    pub fn events(&self) -> &IndexMap<EventId, Event> {
        &self.events
    }

    /// The event `id`, if this journal holds it.
    pub fn event(&self, id: EventId) -> Option<&Event> {
        self.events.get(&id)
    }

    /// Each machine's events, in its append order.
    pub fn order(&self) -> &IndexMap<String, Vec<EventId>> {
        &self.order
    }

    /// Ids found with several contents, each content once, in the order found.
    pub fn quarantined(&self) -> &IndexMap<EventId, Vec<Value>> {
        &self.quarantined
    }

    /// Whether every record added has been judged.
    pub fn is_settled(&self) -> bool {
        self.settled
    }
}

/// The ids an event lists as causes that have the UUID shape, when its `deps` is a list.
fn listed_ids(event: &Value) -> Vec<Value> {
    event
        .get("deps")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter(|dep| dep.as_str().and_then(EventId::parse).is_some())
        .cloned()
        .collect()
}

/// The stand-in for an id that cannot be used: it keeps the id's place and causes, and applies
/// nothing.
fn placeholder(
    id: EventId,
    op: &str,
    replica: &str,
    deps: Vec<Value>,
    last: (&str, Value),
) -> Value {
    let mut members = Object::new();
    members.insert("id".into(), Value::from(id.to_string()));
    members.insert("op".into(), Value::from(op));
    members.insert("replica".into(), Value::from(replica));
    members.insert("deps".into(), Value::Array(deps));
    members.insert("data".into(), Value::Object(Object::new()));
    members.insert(last.0.into(), last.1);
    Value::Object(members)
}

/// Whether two records hold the same content, as Python's `canonical` (sorted keys, compact) tells
/// them apart: object members in any order, floats by their bits (`-0.0` is not `0.0`), the rest
/// exactly. Written without recursion, so a value of any depth is compared.
fn same_content(a: &Value, b: &Value) -> bool {
    let mut stack = vec![(a, b)];
    while let Some((a, b)) = stack.pop() {
        match (a, b) {
            (Value::Float(x), Value::Float(y)) if x.to_bits() == y.to_bits() => {}
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                stack.extend(x.iter().zip(y))
            }
            (Value::Object(x), Value::Object(y)) if x.len() == y.len() => {
                for (key, value) in x {
                    let Some(other) = y.get(key) else {
                        return false;
                    };
                    stack.push((value, other));
                }
            }
            (Value::Null | Value::Bool(_) | Value::Int(_) | Value::Str(_), _) if a == b => {}
            _ => return false,
        }
    }
    true
}

/// An event's content as quarantine compares it (Python's `canonical`): sorted keys, compact.
pub fn canonical(event: &Value) -> String {
    // Only a value built in memory fails to print (an integer too long, nesting too deep); its
    // debug form still tells contents apart.
    json::write(event, Format::CANONICAL).unwrap_or_else(|_| format!("{event:?}"))
}
