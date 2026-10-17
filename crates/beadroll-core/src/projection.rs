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
//!
//! What every event had seen is compacted so now and then, not at every write-back (roll-lqjj.43):
//! a projection extended and written back keeps every event it knew and every event it took in,
//! and is compacted again by the write-back that finds the events taken in since its last
//! compaction more than a [`COMPACTED`]th of those it knew then. Between, it knows events its state
//! no longer names, with their places and what each had seen as replay found them, which nothing
//! asks of it: what applying an event asks, and what a state read alone answers with
//! ([`State::conflicts`] and who wrote what it names), is of the events the state names, and the
//! answers about those are the same whatever else it knows.
//!
//! Each issue's texts are kept apart (roll-lqjj.6), the metadata of its edges' additions
//! (roll-lqjj.39) and its ownership history (the claims that ended, the claims that lost and the
//! ends that came too late, roll-lqjj.40) among them: [`Projection::encode`] gives the bytes of the
//! texts written nowhere yet, placed after `texts_at` bytes of a store's text file, and a state
//! read back has every issue's texts there, until the store reads in those a command needs
//! ([`State::wanted_texts`], [`State::read_texts`]), and before applying records, those
//! [`Projection::about`] lists.
//!
//! A projection read back keeps the bytes it was read from, and writing it back copies the record
//! of every issue and event that did not change since (roll-lqjj.44, `parts.rs`): its issues mark
//! what changes them ([`crate::state::Issues`]), and events are only ever added between
//! compactions.

pub(crate) mod codec;
mod extend;
mod parts;
mod state_codec;
mod text_codec;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{DefaultHasher, Hasher as _};

use crate::id::EventId;
use crate::journal::Journal;
use crate::json::Value;
use crate::replay::replayed;
use crate::state::{KeySpace, State, TextPlace, Texts};

use text_codec::read_texts;

pub use codec::{Part, Unreadable};
pub use extend::Extended;
pub use parts::{Parts, decode_state};

/// The version of the bytes [`Projection::encode`] writes. Bytes of another are not read.
pub const ENCODING: u32 = 11;

/// How many times the events a projection took in of what every event had seen since it was last
/// compacted go into the events it knew then, at most, before a write-back compacts it again
/// (roll-lqjj.43): between compactions it knows the events its state named and the frontier when
/// it was last compacted and at most an eighth as many again, and a write-back writes it as it is,
/// without finding what the state names or cutting down what it knows.
pub const COMPACTED: usize = 8;

/// A projection's bytes: its state, which a command that only reads needs alone, its index, what
/// applying more events needs, and the texts it has that are written nowhere yet.
#[derive(Clone, Debug)]
pub struct Encoded {
    /// The state, each issue with where its texts are, and of its events only who wrote those a
    /// state's conflicts name and where machines' journals broke (roll-lqjj.35).
    pub state: Vec<u8>,
    /// Every event it knows of what every event had seen, with its causes, machine and actor, and
    /// what each had seen (those the state named and the frontier when it was last compacted, and
    /// every event taken in since), how many it knew then (roll-lqjj.43), the frontier, every event id, each machine's journal length and latest event, the claims
    /// takeovers superseded, and whether events can be applied after.
    pub index: Vec<u8>,
    /// The texts written nowhere yet, each issue's at the place the state gives it: to append to
    /// the text file the state's places are in.
    pub texts: Vec<u8>,
}

/// A 128-bit sum of `bytes`: SipHash-1-3 twice, each started apart. It finds damage and change,
/// not forgery: the store is private.
pub fn sum(bytes: &[u8]) -> [u8; 16] {
    sum_of(&[bytes])
}

/// [`sum`] of the bytes `slices` hold, one after another, as they are: what a part written back in
/// pieces sums to ([`Part::slices`], roll-lqjj.51).
pub fn sum_of(slices: &[&[u8]]) -> [u8; 16] {
    let half = |start: u8| {
        let mut hasher = DefaultHasher::new();
        hasher.write_u8(start);
        for bytes in slices {
            hasher.write(bytes);
        }
        hasher.finish()
    };
    let mut out = [0; 16];
    let halves = (half(0).to_le_bytes().into_iter()).chain(half(1).to_le_bytes());
    for (slot, byte) in out.iter_mut().zip(halves) {
        *slot = byte;
    }
    out
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
    /// The state, what every event had seen kept for the events it named and the frontier when last
    /// compacted, and every event taken in since ([`Projection::compacted`]).
    pub(crate) state: State,
    /// The frontier: the events nothing builds on, by id.
    pub(crate) heads: Vec<EventId>,
    /// Every event the journal holds, by id: a record of one of them is not a new event. A set,
    /// so that a batch's ids go in where they belong at a cost of their own, whatever the clocks
    /// that minted them make of their order (roll-lqjj.17).
    pub(crate) ids: BTreeSet<EventId>,
    /// Each machine's journal length: the position of its next record.
    pub(crate) lengths: HashMap<String, u32>,
    /// Each machine's latest event in its journal, by id: when it last wrote, which a machine's
    /// last record need not say, since ids are minted from a clock that can go back (roll-lqjj.31).
    pub(crate) lasts: HashMap<String, EventId>,
    /// The claims takeovers superseded.
    pub(crate) superseded: HashSet<EventId>,
    /// Whether events can be applied after it: nothing in its journal waits for a cause, and
    /// nothing replay made of it can change with a record still to come but by the record applying
    /// after it (`replay::Replayed::unsettled`). Whatever else replay reports stays as it is,
    /// whatever comes after (`extend.rs`, roll-lqjj.28).
    pub(crate) open: bool,
    /// How many events the knowledge held when it was last compacted to the events the state named
    /// and the frontier (roll-lqjj.43): every event taken in since is held too, until a write-back
    /// compacts it again ([`Projection::compaction_due`]).
    pub(crate) compacted: usize,
    /// The bytes it was read back from, to copy what did not change since when it is written back
    /// (roll-lqjj.44): none for a projection replay made or one that let go of them.
    pub(crate) kept: Option<std::sync::Arc<parts::Kept>>,
    /// The ids taken in since it was read back, which a write-back merges into the ones it read.
    pub(crate) added: Vec<EventId>,
}

/// Replays `journal` into a projection. Fails only as [`crate::replay::replay`] does.
pub fn project(mut journal: Journal) -> Result<Projection, KeySpace> {
    journal.settle();
    let heads = journal.frontier();
    let ids: BTreeSet<EventId> = journal.events().keys().copied().collect();
    let lengths = (journal.order().iter())
        .map(|(machine, ids)| (machine.clone(), small(ids.len())))
        .collect();
    let lasts = (journal.order().iter())
        .filter_map(|(machine, ids)| Some((machine.clone(), *ids.iter().max()?)))
        .collect();
    let (mut state, replayed) = replayed(journal)?;
    let open = state.pending.is_empty() && !replayed.unsettled;
    let mut kept = named(&state);
    kept.extend(heads.iter().copied());
    state.knowledge = state.knowledge.kept(&kept);
    let compacted = state.knowledge.events.len();
    Ok(Projection {
        state,
        heads,
        ids,
        lengths,
        lasts,
        superseded: replayed.superseded,
        open,
        compacted,
        kept: None,
        added: Vec::new(),
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

    /// How many events the journal holds: every id, an attempt file's and a stand-in's among them.
    pub fn event_count(&self) -> usize {
        self.ids.len()
    }

    /// Each machine's journal, in the order of their names: how many records it holds, and its
    /// latest event, by id.
    pub fn journals(&self) -> Vec<(&str, u32, Option<EventId>)> {
        let mut out: Vec<(&str, u32, Option<EventId>)> = (self.lengths.iter())
            .map(|(machine, length)| (machine.as_str(), *length, self.lasts.get(machine).copied()))
            .collect();
        out.sort_unstable_by(|a, b| a.0.cmp(b.0));
        out
    }

    /// How many events it knows of what every event had seen (roll-lqjj.43): the events its state
    /// names and the frontier, as it was when last compacted, and every event taken in since.
    pub fn known(&self) -> usize {
        self.state.knowledge.events.len()
    }

    /// How many events it knew of what every event had seen when that was last compacted.
    pub fn compacted(&self) -> usize {
        self.compacted
    }

    /// Whether writing it back compacts what every event had seen to the events its state names and
    /// the frontier: once the events taken in since it was last compacted are more than a
    /// [`COMPACTED`]th of those it knew then (roll-lqjj.43). At an eighth exactly, it is written as
    /// it is; one event more, and it is compacted.
    pub fn compaction_due(&self) -> bool {
        let since = self.known().saturating_sub(self.compacted);
        since.saturating_mul(COMPACTED) > self.compacted
    }

    /// Whether events can be applied after it ([`Projection::extend`]): nothing in its journal waits
    /// for a cause, no takeover was refused for naming an id no event has, and every id with
    /// several contents is placed by all its causes.
    pub fn extendable(&self) -> bool {
        self.open
    }

    /// The state, to read texts into.
    pub fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }

    /// The texts [`Projection::encode`] placed, written where it placed them.
    pub fn placed(&mut self, places: &[(EventId, TextPlace)]) {
        for (uid, place) in places {
            if let Some(issue) = self.state.issues.get_mut(uid)
                && matches!(issue.texts, Texts::Here)
            {
                issue.texts = Texts::Read(*place);
            }
        }
    }
}

impl State {
    /// Where the texts of the issues among `uids` that have them only in a text file are: each
    /// issue once, however many times `uids` names it, in the order of their places.
    pub fn wanted_texts(
        &self,
        uids: impl IntoIterator<Item = EventId>,
    ) -> Vec<(EventId, TextPlace)> {
        let mut named = HashSet::new();
        let mut out: Vec<(EventId, TextPlace)> = (uids.into_iter())
            .filter(|uid| named.insert(*uid))
            .filter_map(|uid| match &self.issues.get(&uid)?.texts {
                Texts::There(stored) => Some((uid, stored.place)),
                _ => None,
            })
            .collect();
        out.sort_by_key(|(_, place)| place.at);
        out
    }

    /// Where the texts of every issue that has them only in a text file are, in the order they
    /// were written.
    pub fn all_wanted_texts(&self) -> Vec<(EventId, TextPlace)> {
        self.wanted_texts(self.issues.keys().copied())
    }

    /// How many bytes of a text file the texts of its issues take where they are placed: what of it
    /// is still read.
    pub fn placed_text_bytes(&self) -> u64 {
        (self.issues.values())
            .map(|issue| match &issue.texts {
                Texts::Read(place) => u64::from(place.len),
                Texts::There(stored) => u64::from(stored.place.len),
                Texts::Here => 0,
            })
            .sum()
    }

    /// Has every issue's texts written anew, as one written nowhere yet: `false`, and nothing
    /// changed, when one has them only in a text file.
    pub fn write_texts_anew(&mut self) -> bool {
        if !self.issues.values().all(|issue| issue.texts.here()) {
            return false;
        }
        for issue in self.issues.values_mut() {
            issue.texts = Texts::Here;
        }
        true
    }

    /// Reads in the texts of issue `uid` from `bytes`, read at `place` of the text file: they must
    /// be the ones written there (their length and sum) and the issue's. Nothing changes when they
    /// are not.
    pub fn read_texts(
        &mut self,
        uid: EventId,
        place: TextPlace,
        bytes: &[u8],
    ) -> Result<(), Unreadable> {
        let issue = self.issues.read_in(&uid).ok_or(Unreadable("an issue"))?;
        if issue.texts.here() {
            return Ok(()); // read in already
        }
        if u32::try_from(bytes.len()).ok() != Some(place.len) || sum(bytes) != place.sum {
            return Err(Unreadable("damaged texts"));
        }
        read_texts(issue, bytes, place)
    }
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
        if let Texts::There(stored) = &issue.texts {
            out.extend(stored.tags.iter().flat_map(|(_, ids)| ids.iter().copied()));
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

/// The events a state read alone asks who wrote: those behind each issue's registers with several
/// entries ([`state_codec::asked`]).
#[cfg(test)]
pub(crate) fn asked(state: &State) -> HashSet<EventId> {
    (state.issues.values())
        .flat_map(|issue| state_codec::asked(issue))
        .collect()
}

/// `n` as a position (a journal holds fewer than 2³² events).
fn small(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
pub(crate) mod tests;
