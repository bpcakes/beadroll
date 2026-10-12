//! A replayed state that more events can be applied to (roll-lqjj.2, step 3 of epic roll-lqjj):
//! what a store keeps as its projection, so that a command need not replay the whole journal. It
//! is a cache, never a source.
//!
//! [`project`] replays a journal and keeps, of what every event had seen, only the events its state
//! still names: each issue's create, the events behind its registers and its label and dependency
//! tags, its removals and its claims, and the frontier. Applying an event after all of a journal's
//! asks about nothing else (`saw_at`, `saw` and `who` in `replay/apply.rs`). [`Projection::extend`]
//! applies records appended to that journal when replay would apply them after every event it
//! holds, and as it would; when it cannot know that, it declines, and the caller replays.

pub(crate) mod codec;
mod extend;
mod state_codec;

use std::collections::{HashMap, HashSet};

use crate::id::EventId;
use crate::journal::Journal;
use crate::json::Value;
use crate::replay::replayed;
use crate::state::{KeySpace, Knowledge, State};

use codec::{Reader, Writer};
use state_codec::{read_state, write_state};

pub use codec::Unreadable;
pub use extend::Extended;

/// The version of the bytes [`Projection::encode`] writes. Bytes of another are not read.
pub const ENCODING: u32 = 1;

/// A projection's bytes: its state, which a command that only reads needs alone, and its index,
/// what applying more events needs.
#[derive(Clone, Debug)]
pub struct Encoded {
    /// The state.
    pub state: Vec<u8>,
    /// What every event the state names had seen, the frontier, every event id, each machine's
    /// journal length, the claims takeovers superseded, and whether events can be applied after.
    pub index: Vec<u8>,
}

/// A record of a journal's files, to apply: an event as it was read, the machine whose folder held
/// it, the file, and whether it is one of that machine's journal's records (an attempt file's are
/// not).
#[derive(Clone, Debug)]
pub struct Record {
    /// The event, as read.
    pub value: Value,
    /// The machine whose folder held it.
    pub replica: String,
    /// The file it was read from.
    pub origin: String,
    /// Whether it is a record of that machine's journal.
    pub sequenced: bool,
}

/// A journal's state, replayed, with what applying the events that come after it needs.
#[derive(Clone, Debug)]
pub struct Projection {
    /// The state, what every event had seen kept for the events it still names.
    pub(crate) state: State,
    /// The frontier: the events nothing builds on, by id.
    pub(crate) heads: Vec<EventId>,
    /// Every event the journal holds, by id: a record of one of them is not a new event.
    pub(crate) ids: Vec<EventId>,
    /// Each machine's journal length: the position of its next record.
    pub(crate) lengths: HashMap<String, u32>,
    /// The claims takeovers superseded.
    pub(crate) superseded: HashSet<EventId>,
    /// Whether events can be applied after it: the journal had nothing quarantined, unreadable,
    /// misplaced, inert or waiting for a cause, which replay reports in an order of their own.
    pub(crate) open: bool,
}

/// Replays `journal` into a projection. Fails only as [`crate::replay::replay`] does.
pub fn project(mut journal: Journal) -> Result<Projection, KeySpace> {
    journal.settle();
    let heads = journal.frontier();
    let mut ids: Vec<EventId> = journal.events().keys().copied().collect();
    ids.sort_unstable();
    let lengths = (journal.order().iter())
        .map(|(machine, ids)| (machine.clone(), small(ids.len())))
        .collect();
    let (mut state, superseded) = replayed(journal)?;
    let open = state.integrity.is_empty() && state.pending.is_empty();
    let mut kept = named(&state);
    kept.extend(heads.iter().copied());
    state.knowledge = state.knowledge.kept(&kept);
    Ok(Projection {
        state,
        heads,
        ids,
        lengths,
        superseded,
        open,
    })
}

impl Projection {
    /// The state.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// The state, the projection let go of.
    pub fn into_state(self) -> State {
        self.state
    }

    /// The frontier: the events nothing builds on, by id. A new event's causes.
    pub fn heads(&self) -> &[EventId] {
        &self.heads
    }

    /// Whether events can be applied after it ([`Projection::extend`]): its journal had nothing
    /// quarantined, unreadable, misplaced, inert or waiting.
    pub fn extendable(&self) -> bool {
        self.open
    }

    /// The projection as bytes. Of what every event had seen it keeps the events the state still
    /// names, and the frontier.
    pub fn encode(&self) -> Encoded {
        let mut named = named(&self.state);
        named.extend(self.heads.iter().copied());
        let kept = self.state.knowledge.kept(&named);
        let mut w = Writer::default();
        write_state(&mut w, &self.state);
        kept.write_events(&mut w);
        let state = w.finish();
        let mut w = Writer::default();
        kept.write_index(&mut w);
        w.ids(&self.heads);
        w.ids(&self.ids);
        let mut lengths: Vec<(&String, &u32)> = self.lengths.iter().collect();
        lengths.sort();
        w.count(lengths.len());
        for (machine, length) in lengths {
            w.text(machine);
            w.number(u64::from(*length));
        }
        let mut superseded: Vec<EventId> = self.superseded.iter().copied().collect();
        superseded.sort_unstable();
        w.ids(&superseded);
        w.flag(self.open);
        Encoded {
            state,
            index: w.finish(),
        }
    }

    /// The projection [`Projection::encode`] wrote as these bytes.
    pub fn decode(encoded: &Encoded) -> Result<Projection, Unreadable> {
        let mut state = decode_state(&encoded.state)?;
        let mut r = Reader::new(&encoded.index);
        state.knowledge.read_index(&mut r)?;
        let heads = r.ids()?;
        if !(heads.iter()).all(|head| state.knowledge.events.contains_key(head)) {
            return Err(Unreadable("the frontier"));
        }
        let ids = r.ids()?;
        // looked up by halves: in order, each once
        if !ids.windows(2).all(|pair| matches!(pair, [a, b] if a < b)) {
            return Err(Unreadable("the ids"));
        }
        let mut lengths = HashMap::new();
        for _ in 0..r.count()? {
            let machine = r.text()?;
            lengths.insert(machine, r.small()?);
        }
        let superseded = r.ids()?.into_iter().collect();
        let open = r.flag()?;
        r.done()?;
        Ok(Projection {
            state,
            heads,
            ids,
            lengths,
            superseded,
            open,
        })
    }
}

/// The state [`Projection::encode`] wrote as `bytes` ([`Encoded::state`]): all a command that only
/// reads needs. Of its events it knows the machine and actor of those it names, not what each had
/// seen.
pub fn decode_state(bytes: &[u8]) -> Result<State, Unreadable> {
    let mut r = Reader::new(bytes);
    let mut state = read_state(&mut r)?;
    state.knowledge = Knowledge::read_events(&mut r)?;
    r.done()?;
    Ok(state)
}

/// Every event `state` still names, which applying an event after all of them can ask about: each
/// issue's create, the events behind its registers and its label and dependency tags, its removals,
/// and its claim and offline claims.
fn named(state: &State) -> HashSet<EventId> {
    let mut out = HashSet::new();
    for issue in state.issues.values() {
        out.insert(issue.uid);
        for tags in issue.reg.values() {
            out.extend(tags.iter().map(|(id, _)| *id));
        }
        for tags in issue.label_tags.values() {
            out.extend(tags.iter().copied());
        }
        for tags in issue.dep_tags.values() {
            out.extend(tags.iter().map(|(id, _)| *id));
        }
        for removals in issue.retracted.values() {
            out.extend(removals.iter().copied());
        }
        out.extend(issue.claim.as_ref().map(|claim| claim.id));
        out.extend(issue.provisional_claims.iter().map(|offline| offline.id));
    }
    out
}

/// `n` as a position (a journal holds fewer than 2³² events).
fn small(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
