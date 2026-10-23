//! An event as a journal keeps it (roll-dih8.11): what replay and `log` read of a record, read once
//! when the record is added. The record's JSON object is not kept, so that a journal costs a few
//! hundred bytes an event rather than kilobytes: its content is a digest, which tells two records
//! of one id apart (`content`), and the machines, issues, actors and places that events repeat are
//! held once ([`Shared`]).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, LazyLock};

use crate::event::{Data, Envelope, FORMAT};
use crate::id::EventId;
use crate::json::{self, Format, Value};
use crate::op::{IntegerField, Op};
use crate::text::str_of;

use super::content::{Digest, Digester};

/// An event as replay sees it: a valid event, or the stand-in for an id that cannot be used (`op`
/// `quarantined` or `invalid`). Journals share their events, so a clone costs a count.
#[derive(Clone, Debug, PartialEq)]
pub struct Event(Arc<Read>);

/// What was read of one record: what a replayed state keeps of it, its causes, machine and actor,
/// and the rest, which replay takes and lets go of once it has applied the event.
#[derive(Debug, PartialEq)]
struct Read {
    id: EventId,
    /// Its causes, as listed: the members of `deps` that are UUIDs.
    deps: Box<[EventId]>,
    /// The machine it names (`replica`), or `""`.
    replica: Arc<str>,
    /// Its `actor`, as written.
    actor: Option<Arc<Value>>,
    /// What it does and how it was found; gone once replay has taken it.
    rest: Option<Box<Rest>>,
}

/// What an event does, and how its record was found.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Rest {
    pub(crate) op: Op,
    /// The issue it names, when that is text.
    pub(crate) issue: Option<Arc<str>>,
    /// Its `where`, as written.
    pub(crate) place: Option<Arc<Value>>,
    /// Whether its payload says it came from an import (`data.imported`).
    pub(crate) imported: bool,
    /// What is wrong with it, if anything: few records have anything.
    trouble: Option<Box<Trouble>>,
    /// Its content, as this journal tells contents apart.
    digest: Digest,
    source: Arc<Source>,
    /// The how-manyth record it was, as records were added.
    counter: u64,
}

/// Where records were found: the journal of machine `replica` (which a record may not name), the
/// file, and whether its records have places in that journal's order.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Source {
    pub(crate) replica: Arc<str>,
    pub(crate) origin: Arc<str>,
    pub(crate) sequenced: bool,
}

/// What is wrong with a record.
#[derive(Clone, Debug, PartialEq)]
enum Trouble {
    /// It is not a valid event (`event_problem`).
    Invalid(String),
    /// It is a valid event that replay cannot apply all the same: a format this bead does not
    /// read, or a payload it does not write.
    Flaw(String),
}

/// What an event replay has taken the rest of answers: a placeholder that applies nothing. Only
/// replay takes the rest, and it reads it from what it took.
static TAKEN: LazyLock<Rest> = LazyLock::new(|| Rest {
    op: Op::Invalid,
    issue: None,
    place: None,
    imported: false,
    trouble: None,
    digest: [0; 2],
    source: Arc::new(Source {
        replica: Arc::from(""),
        origin: Arc::from(""),
        sequenced: false,
    }),
    counter: 0,
});

/// Whether an integer field equals `n`, mapping true to 1 and false to 0.
fn same_int(value: &Value, n: i64) -> bool {
    IntegerField::read(value).is_some_and(|value| value == IntegerField::from(n))
}

impl Rest {
    /// Why replay cannot apply this valid event: its format or its payload.
    pub(crate) fn flaw(&self) -> Option<&str> {
        match self.trouble.as_deref() {
            Some(Trouble::Flaw(flaw)) => Some(flaw),
            _ => None,
        }
    }
}

impl Event {
    /// What the record read as `envelope`, the `counter`-th added and found in `source`, holds
    /// under `id`, its content's digest being `digest`.
    pub(crate) fn read(
        id: EventId,
        envelope: Envelope,
        digest: Digest,
        source: &Arc<Source>,
        counter: u64,
        shared: &mut Shared,
    ) -> Event {
        let trouble = match envelope.problem() {
            Some(problem) => Some(Trouble::Invalid(problem)),
            None => match envelope.v.as_ref().filter(|v| !same_int(v, FORMAT)) {
                Some(version) => Some(format!(
                    "event format v{} is not one this version of bead reads",
                    str_of(version)
                )),
                None => envelope.payload_problem(),
            }
            .map(Trouble::Flaw),
        };
        let Envelope {
            deps,
            op,
            replica,
            data,
            issue,
            actor,
            place,
            ..
        } = envelope;
        let data = match data {
            Data::Object(data) => data,
            _ => Default::default(),
        };
        let rest = Rest {
            imported: data.get("imported").is_some_and(Value::truthy),
            op: Op::read(op.as_deref().unwrap_or_default(), data),
            issue: issue.as_str().map(|issue| shared.text(issue)),
            place: place.as_ref().map(|place| shared.value(place)),
            trouble: trouble.map(Box::new),
            digest,
            source: Arc::clone(source),
            counter,
        };
        Event(Arc::new(Read {
            id,
            deps: deps.into_boxed_slice(),
            replica: shared.text(replica.as_deref().unwrap_or_default()),
            actor: actor.as_ref().map(|actor| shared.value(actor)),
            rest: Some(Box::new(rest)),
        }))
    }

    /// An event replay has applied, as a replayed state keeps it (its id, causes, machine and
    /// actor), read back from a projection.
    pub(crate) fn applied(
        id: EventId,
        deps: Box<[EventId]>,
        replica: Arc<str>,
        actor: Option<Arc<Value>>,
    ) -> Event {
        Event(Arc::new(Read {
            id,
            deps,
            replica,
            actor,
            rest: None,
        }))
    }

    /// The stand-in for an id that cannot be used, found as `placement` was: it keeps the id's
    /// place in that journal and the causes `deps`, names the issue `issue`, and applies nothing.
    pub(crate) fn stand_in(
        placement: &Event,
        op: Op,
        deps: Box<[EventId]>,
        issue: Option<Arc<str>>,
    ) -> Event {
        let found = placement.rest();
        let source = Arc::clone(&found.source);
        let rest = Rest {
            op,
            issue,
            place: None,
            imported: false,
            trouble: None,
            digest: [0; 2],
            source: Arc::clone(&source),
            counter: found.counter,
        };
        Event(Arc::new(Read {
            id: placement.0.id,
            deps,
            replica: Arc::clone(&source.replica),
            actor: None,
            rest: Some(Box::new(rest)),
        }))
    }

    /// What it does and how it was found.
    fn rest(&self) -> &Rest {
        self.0.rest.as_deref().unwrap_or(&TAKEN)
    }

    /// What it does, taken for replay to apply: moved out when this is the event's only handle,
    /// so that it goes once applied; copied when a journal that shares the event lives on.
    pub(crate) fn take_rest(&mut self) -> Box<Rest> {
        match Arc::get_mut(&mut self.0).and_then(|read| read.rest.take()) {
            Some(rest) => rest,
            None => Box::new(self.rest().clone()),
        }
    }

    /// Its id.
    pub fn id(&self) -> EventId {
        self.0.id
    }

    /// The events it names as causes, as listed (`deps`).
    pub fn deps(&self) -> &[EventId] {
        &self.0.deps
    }

    /// Its operation, with its payload read as replay reads it.
    pub fn op(&self) -> &Op {
        &self.rest().op
    }

    /// The name of its operation, as written (`quarantined` and `invalid` for a stand-in).
    pub fn op_name(&self) -> &str {
        self.op().name()
    }

    /// The machine that wrote it.
    pub fn replica(&self) -> &str {
        &self.0.replica
    }

    /// The issue it is about, if it names one.
    pub fn issue(&self) -> Option<&str> {
        self.rest().issue.as_deref()
    }

    /// Its `actor`, as written.
    pub fn actor(&self) -> Option<&Value> {
        self.0.actor.as_deref()
    }

    /// The machine it names, shared with every event that names it.
    pub(crate) fn shared_replica(&self) -> &Arc<str> {
        &self.0.replica
    }

    /// Its `actor`, shared with every event written alike.
    pub(crate) fn shared_actor(&self) -> Option<&Arc<Value>> {
        self.0.actor.as_ref()
    }

    /// Its `where`, as written.
    pub fn place(&self) -> Option<&Value> {
        self.rest().place.as_deref()
    }

    /// Whether the payload's `imported` member is truthy.
    pub fn imported(&self) -> bool {
        self.rest().imported
    }

    /// Why it is not a valid event, if it is not.
    pub(crate) fn problem(&self) -> Option<&str> {
        match self.rest().trouble.as_deref() {
            Some(Trouble::Invalid(problem)) => Some(problem),
            _ => None,
        }
    }

    /// Its content's digest: equal for two records exactly when they hold the same content.
    pub(crate) fn digest(&self) -> &Digest {
        &self.rest().digest
    }

    /// Where its record was found.
    pub(crate) fn source(&self) -> &Arc<Source> {
        &self.rest().source
    }

    /// The how-manyth record it was.
    pub(crate) fn counter(&self) -> u64 {
        self.rest().counter
    }

    /// Whether it is a valid event found in its own machine's journal.
    pub(crate) fn own(&self) -> bool {
        self.problem().is_none() && self.0.replica == self.rest().source.replica
    }
}

/// The texts and values a journal's events repeat, each held once: machines, issues and files,
/// actors and places. A value is shared only with one written alike, its members in the same order,
/// so that each prints as it was written. Also the room records are digested in.
#[derive(Clone, Debug, Default)]
pub(crate) struct Shared {
    texts: HashSet<Arc<str>>,
    values: HashMap<String, Arc<Value>>,
    /// The values held last: an event most often names the actor and the place the one before did.
    recent: VecDeque<Arc<Value>>,
    pub(super) digester: Digester,
}

/// Whether `value` holds no other value.
fn scalar(value: &Value) -> bool {
    !matches!(value, Value::Array(_) | Value::Object(_))
}

/// Whether `a`, a scalar, is written as `b` is.
fn same_scalar(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::Null | Value::Bool(_) | Value::Int(_) | Value::Str(_), _) => a == b,
        _ => false,
    }
}

/// How many values [`Shared::recent`] keeps.
const RECENT: usize = 4;

/// Whether `a` and `b` are written alike: equal, with their objects' members in the same order and
/// floats of the same bits. Compared without recursion, so a value of any depth is compared; a flat
/// object, as actors and places are, without any room taken.
fn written_alike(a: &Value, b: &Value) -> bool {
    if let (Value::Object(x), Value::Object(y)) = (a, b)
        && x.len() == y.len()
        && x.values().all(scalar)
    {
        return x
            .iter()
            .zip(y.iter())
            .all(|((key, value), (other_key, other))| {
                key == other_key && same_scalar(value, other)
            });
    }
    let mut stack = vec![(a, b)];
    while let Some((a, b)) = stack.pop() {
        match (a, b) {
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                stack.extend(x.iter().zip(y.iter()));
            }
            (Value::Object(x), Value::Object(y)) if x.len() == y.len() => {
                for ((key, value), (other_key, other)) in x.iter().zip(y.iter()) {
                    if key != other_key {
                        return false;
                    }
                    stack.push((value, other));
                }
            }
            (Value::Float(x), Value::Float(y)) if x.to_bits() == y.to_bits() => {}
            (Value::Null | Value::Bool(_) | Value::Int(_) | Value::Str(_), _) if a == b => {}
            _ => return false,
        }
    }
    true
}

impl Shared {
    /// `text`, held once.
    pub(crate) fn text(&mut self, text: &str) -> Arc<str> {
        if let Some(held) = self.texts.get(text) {
            return Arc::clone(held);
        }
        let held: Arc<str> = Arc::from(text);
        self.texts.insert(Arc::clone(&held));
        held
    }

    /// `value`, held once with every value written alike.
    pub(crate) fn value(&mut self, value: &Value) -> Arc<Value> {
        if let Some(held) = self.recent.iter().find(|held| written_alike(held, value)) {
            return Arc::clone(held);
        }
        let held = match json::write(value, Format::COMPACT) {
            Ok(written) => Arc::clone(
                self.values
                    .entry(written)
                    .or_insert_with(|| Arc::new(value.clone())),
            ),
            Err(_) => Arc::new(value.clone()), // too long or too deep to print: kept alone
        };
        if self.recent.len() == RECENT {
            self.recent.pop_back();
        }
        self.recent.push_front(Arc::clone(&held));
        held
    }
}
