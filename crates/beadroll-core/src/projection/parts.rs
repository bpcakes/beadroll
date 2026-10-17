//! A projection's two parts as bytes (roll-lqjj.35, roll-lqjj.44): its state, which a command that
//! only reads decodes alone, and its index, which applying more events needs besides. Each is a
//! part of records ([`super::codec::Writer::tabled`]): every issue's record in the state part and
//! every event's in the index refer to the names and values they share by their number in the
//! part's table, which only grows, so that a record means the same wherever it sits.
//!
//! A projection read back keeps the bytes it was read from ([`Kept`]), and writing it back copies
//! what did not change since: the record of every issue nothing marked
//! ([`crate::state::Issues::changed`]), the records of the events read back, which between two
//! compactions never change and are only added after, and the chain sets and positions their
//! clocks share, which only grow; the ids it read, the batch's merged in where they sort. The rest
//! is written: an issue changed or new, an event new, and what lies beside them. A compaction
//! renumbers the events, so the index is written whole then, its table made anew; the state part's
//! table is made anew once it holds more than an eighth more entries than when it last was.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use crate::id::EventId;
use crate::state::{Knowledge, State, TextPlace, Texts};

use super::codec::{Reader, Table, Unreadable, Writer};
use super::state_codec::{Records, read_state, write_state};
use super::text_codec::write_texts;
use super::{Encoded, Projection, named, sum};

/// What a projection read back keeps of the bytes it was read from, to copy from when it is written
/// back: its two parts, where each issue's record begins in the state part, and where its event
/// records, chain sets, positions and ids are in its index, with how many of each, and the tables
/// of both parts.
pub(crate) struct Kept {
    state: Vec<u8>,
    index: Vec<u8>,
    issues: Vec<u32>,
    keys: (Range<usize>, usize),
    pub(super) state_table: Table,
    /// How many entries the state part's table had when it was last made anew.
    pub(super) fresh: usize,
    pub(super) index_table: Table,
    events: (Range<usize>, usize),
    places: Range<usize>,
    clocks: Range<usize>,
    sets: (Range<usize>, usize),
    positions: (Range<usize>, usize),
    ids: (Range<usize>, usize),
}

impl std::fmt::Debug for Kept {
    /// Its sizes, not its bytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Kept {{ state: {} bytes, index: {} bytes, {} issues, {} events }}",
            self.state.len(),
            self.index.len(),
            self.issues.len().saturating_sub(1),
            self.events.1
        )
    }
}

/// What a write-back copies (roll-lqjj.44): what did not change since the projection was read back,
/// or, as the tests compare it with, nothing, every record written against the same tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Copying {
    Unchanged,
    #[cfg(test)]
    Nothing,
}

/// How many entries a state part's table may hold beyond as many as when it was last made anew,
/// besides an eighth more, before it is made anew: a write-back adds the names and values of the
/// issues it writes when no record holds them yet, and a few dozen cost nothing to read.
pub(super) const TABLE_SLACK: usize = 64;

/// How much of a part read back a write-back reserves beyond its size, for what it adds: a 64th, so
/// that the part's room is seldom grown and the write-back's heap peak, which holds the bytes read
/// back beside the ones written, is not raised by room it does not use.
const SPARE: usize = 64;

impl Projection {
    /// The projection as bytes, the texts written nowhere yet placed from `texts_at`, the length of
    /// the text file they are to be appended to, and the places it gave them: once both are written,
    /// [`Projection::placed`] has the issues know. Of what every event had seen it keeps, when it is
    /// compacted ([`Projection::compaction_due`]), the events the state still names and the
    /// frontier, and otherwise every event it knows: those, as they were when it was last
    /// compacted, and every event taken in since (roll-lqjj.43). What did not change since it was
    /// read back is copied as it was read (roll-lqjj.44).
    pub fn encode(&self, texts_at: u64) -> (Encoded, Vec<(EventId, TextPlace)>) {
        self.encode_with(texts_at, self.compaction_due())
    }

    /// [`Projection::encode`], what every event had seen cut down to the events the state names and
    /// the frontier when `compact`, and written as it is otherwise, whatever
    /// [`Projection::compaction_due`] says: what the tests compare. Never compacted, a projection
    /// would keep every event it took in, the count of its last compaction carried forward.
    pub(crate) fn encode_with(
        &self,
        texts_at: u64,
        compact: bool,
    ) -> (Encoded, Vec<(EventId, TextPlace)>) {
        self.encode_copying(texts_at, compact, Copying::Unchanged)
    }

    /// [`Projection::encode_with`], copying what `copying` says from what it was read back from.
    pub(crate) fn encode_copying(
        &self,
        texts_at: u64,
        compact: bool,
        copying: Copying,
    ) -> (Encoded, Vec<(EventId, TextPlace)>) {
        let (mut texts, mut places) = (Vec::new(), Vec::new());
        for issue in self.state.issues.values() {
            if matches!(issue.texts, Texts::Here) {
                let bytes = write_texts(issue);
                places.push((
                    issue.uid,
                    TextPlace {
                        at: texts_at.saturating_add(texts.len() as u64),
                        len: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
                        sum: sum(&bytes),
                    },
                ));
                texts.extend_from_slice(&bytes);
            }
        }
        let given: HashMap<EventId, TextPlace> = places.iter().copied().collect();
        let kept = self.kept.as_deref();
        let state = match kept {
            Some(kept) => {
                let records = (copying == Copying::Unchanged).then_some(Records {
                    bytes: &kept.state,
                    starts: &kept.issues,
                    keys: (
                        kept.state.get(kept.keys.0.clone()).unwrap_or_default(),
                        kept.keys.1,
                    ),
                });
                let (bytes, entries) = self.state_part(&given, kept.state_table.clone(), records);
                let fresh = kept.fresh;
                if entries > fresh + fresh / 8 + TABLE_SLACK {
                    self.state_part(&given, Table::default(), None).0
                } else {
                    bytes
                }
            }
            None => self.state_part(&given, Table::default(), None).0,
        };
        let index = match kept {
            Some(kept) if !compact => {
                let knowledge = &self.state.knowledge;
                self.index_part(knowledge, self.compacted, Some(kept), copying)
            }
            _ => {
                let cut;
                let knowledge = if compact {
                    let mut named = named(&self.state);
                    named.extend(self.heads.iter().copied());
                    cut = self.state.knowledge.kept(&named);
                    &cut
                } else {
                    &self.state.knowledge
                };
                let compacted = if compact {
                    knowledge.events.len()
                } else {
                    self.compacted
                };
                self.index_part(knowledge, compacted, None, copying)
            }
        };
        let encoded = Encoded {
            state,
            index,
            texts,
        };
        (encoded, places)
    }

    /// The state part, its table starting from `table`, the records of issues that did not change
    /// copied from `records` when they are given; and how many entries its table holds.
    fn state_part(
        &self,
        given: &HashMap<EventId, TextPlace>,
        table: Table,
        records: Option<Records<'_>>,
    ) -> (Vec<u8>, usize) {
        let anew = table == Table::default();
        let capacity = records.map_or(0, |records| {
            records.bytes.len() + records.bytes.len() / SPARE
        });
        let mut w = Writer::tabled(table, capacity);
        write_state(&mut w, &self.state, given, records);
        let table = w.table_here();
        let entries = table.names.len() + table.values.len();
        // how many entries the table had when it was last made anew: now, if it is
        let fresh = match self.kept.as_deref() {
            Some(kept) if !anew => kept.fresh,
            _ => entries,
        };
        w.number(fresh as u64);
        (w.finish(), entries)
    }

    /// The index, of `knowledge` and `compacted`, the records of the events read back, the chain
    /// sets and positions and the ids copied from `kept` when it is given and `copying` says so.
    fn index_part(
        &self,
        knowledge: &Knowledge,
        compacted: usize,
        kept: Option<&Kept>,
        copying: Copying,
    ) -> Vec<u8> {
        let table = kept.map_or_else(Table::default, |kept| kept.index_table.clone());
        let copied = kept.filter(|kept| {
            copying == Copying::Unchanged
                && knowledge.events.len() >= kept.events.1
                && knowledge.arena().0 >= kept.sets.1
                && knowledge.arena().1 >= kept.positions.1
        });
        let capacity = kept.map_or(0, |kept| kept.index.len() + kept.index.len() / SPARE);
        let mut w = Writer::tabled(table, capacity);
        w.count(knowledge.events.len());
        let stored = |range: &Range<usize>| copied.and_then(|kept| kept.index.get(range.clone()));
        let from = copied.map_or(0, |kept| kept.events.1);
        w.copy(copied.map_or(&[][..], |kept| stored(&kept.events.0).unwrap_or_default()));
        knowledge.write_records(&mut w, from);
        w.copy(copied.map_or(&[][..], |kept| stored(&kept.places).unwrap_or_default()));
        knowledge.write_places(&mut w, from);
        w.copy(copied.map_or(&[][..], |kept| stored(&kept.clocks).unwrap_or_default()));
        knowledge.write_clocks(&mut w, from);
        knowledge.write_chains(&mut w);
        w.count(compacted);
        w.ids(self.heads.iter());
        match copied.filter(|kept| self.ids.len() == kept.ids.1 + self.added.len()) {
            Some(kept) => self.merged_ids(&mut w, kept),
            None => w.ids(self.ids.iter()),
        }
        let mut lengths: Vec<(&String, &u32)> = self.lengths.iter().collect();
        lengths.sort();
        w.count(lengths.len());
        for (machine, length) in lengths {
            w.text(machine);
            w.number(u64::from(*length));
        }
        let mut lasts: Vec<(&String, &EventId)> = self.lasts.iter().collect();
        lasts.sort();
        w.count(lasts.len());
        for (machine, last) in lasts {
            w.text(machine);
            w.id(*last);
        }
        let mut superseded: Vec<EventId> = self.superseded.iter().copied().collect();
        superseded.sort_unstable();
        w.ids(superseded.iter());
        w.flag(self.open);
        w.table_here();
        let arena = copied.map(|kept| crate::state::Arena {
            counts: (kept.sets.1, kept.positions.1),
            sets: kept.index.get(kept.sets.0.clone()).unwrap_or_default(),
            positions: kept.index.get(kept.positions.0.clone()).unwrap_or_default(),
        });
        knowledge.write_arena(&mut w, arena);
        w.finish()
    }

    /// The ids, as the ascending run `kept` read, with every id taken in since merged in where it
    /// sorts: the run copied between them, sixteen bytes an id.
    fn merged_ids(&self, w: &mut Writer, kept: &Kept) {
        let stored = kept.index.get(kept.ids.0.clone()).unwrap_or_default();
        let id_at = |at: usize| {
            let bytes = stored.get(at * 16..at * 16 + 16).unwrap_or_default();
            u128::from_be_bytes(bytes.try_into().unwrap_or([0xff; 16]))
        };
        let mut added = self.added.clone();
        added.sort_unstable();
        w.count(kept.ids.1 + added.len());
        let mut from = 0;
        for id in added {
            // the first stored id after this one
            let (mut low, mut high) = (from, kept.ids.1);
            while low < high {
                let middle = low + (high - low) / 2;
                if id_at(middle) < id.as_u128() {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            w.copy(stored.get(from * 16..low * 16).unwrap_or_default());
            w.id(id);
            from = low;
        }
        w.copy(stored.get(from * 16..).unwrap_or_default());
    }

    /// The projection [`Projection::encode`] wrote as these bytes, which it keeps to copy from when
    /// it is written back, until it lets go of them ([`Projection::let_go`]).
    pub fn decode(encoded: Encoded) -> Result<Projection, Unreadable> {
        let Encoded { state, index, .. } = encoded;
        let (mut r, mut after, state_table) = Reader::tabled(&state)?;
        let mut starts = Vec::new();
        let (mut read, keys) = read_state(&mut r, Some(&mut starts))?;
        let keys = (keys, read.by_key.len());
        r.done()?;
        let fresh = usize::try_from(after.number()?).map_err(|_| Unreadable("a count"))?;
        after.done()?;
        let (mut r, mut after, index_table) = Reader::tabled(&index)?;
        let mut knowledge = Knowledge::default();
        let n = r.count()?;
        let from = r.at();
        knowledge.read_records(&mut r, n)?;
        let events = (from..r.at(), n);
        let from = r.at();
        knowledge.read_places(&mut r, n)?;
        let places = from..r.at();
        let from = r.at();
        knowledge.read_clocks(&mut r, n)?;
        let clocks = from..r.at();
        let writers = std::mem::take(&mut read.knowledge);
        if !(writers.events.keys()).all(|writer| knowledge.events.contains_key(writer)) {
            return Err(Unreadable("the writers"));
        }
        knowledge.chain_breaks = writers.chain_breaks;
        knowledge.read_chains(&mut r)?;
        let compacted = r.count()?;
        if compacted > knowledge.events.len() {
            return Err(Unreadable("the events last compacted"));
        }
        let heads = r.ids()?;
        if !(heads.iter()).all(|head| knowledge.events.contains_key(head)) {
            return Err(Unreadable("the frontier"));
        }
        let count = r.count()?;
        let from = r.at();
        let ids: Vec<EventId> = (0..count).map(|_| r.id()).collect::<Result<_, _>>()?;
        let id_bytes = (from..r.at(), count);
        // In order, each once: bytes that are not are refused rather than quietly sorted and
        // deduplicated by the set they are read into.
        if !ids.windows(2).all(|pair| matches!(pair, [a, b] if a < b)) {
            return Err(Unreadable("the ids"));
        }
        let mut lengths = HashMap::new();
        for _ in 0..r.count()? {
            let machine = r.text()?;
            lengths.insert(machine, r.small()?);
        }
        let mut lasts = HashMap::new();
        for _ in 0..r.count()? {
            let machine = r.text()?;
            lasts.insert(machine, r.id()?);
        }
        let superseded = r.ids()?.into_iter().collect();
        let open = r.flag()?;
        r.done()?;
        let (sets, positions) = knowledge.read_arena(&mut after)?;
        after.done()?;
        let arena = knowledge.arena();
        read.knowledge = knowledge;
        let kept = Kept {
            issues: starts,
            keys,
            state_table,
            fresh,
            index_table,
            events,
            places,
            clocks,
            sets: (sets, arena.0),
            positions: (positions, arena.1),
            ids: id_bytes,
            state,
            index,
        };
        Ok(Projection {
            state: read,
            heads,
            ids: ids.into_iter().collect(),
            lengths,
            lasts,
            superseded,
            open,
            compacted,
            kept: Some(Arc::new(kept)),
            added: Vec::new(),
        })
    }

    /// Lets go of the bytes it was read back from: written back after, it is written whole. What a
    /// store does once it has written a projection back or decided not to, so that a command that
    /// goes on with it does not hold them.
    pub fn let_go(&mut self) {
        self.kept = None;
        self.added = Vec::new();
    }
}

/// The state [`Projection::encode`] wrote as `bytes` ([`Encoded::state`]): all a command that only
/// reads needs. Of its events it knows only who wrote those behind a register with several
/// entries, which its conflicts name ([`State::who`]), and where machines' journals broke: not
/// their causes, nor what each had seen, nor any other event it names, which only applying more
/// events reads (roll-lqjj.35).
pub fn decode_state(bytes: &[u8]) -> Result<State, Unreadable> {
    let (mut r, mut after, _) = Reader::tabled(bytes)?;
    let (state, _) = read_state(&mut r, None)?;
    r.done()?;
    after.number()?;
    after.done()?;
    Ok(state)
}
