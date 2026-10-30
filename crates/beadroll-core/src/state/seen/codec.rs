//! What a projection's index keeps of what every event had seen, as bytes ([`crate::projection`]),
//! which only applying more events reads: the record of every event it knows (those the state
//! names, and between compactions every event taken in since, roll-lqjj.43), its id, causes,
//! machine and actor, its place and its clock, in the journal's order; the machines, each
//! machine's applicable positions and the chains of unsequenced events; and, after the part's
//! table, the chain sets and positions the clocks share. Between compactions a record never changes
//! and records, chain sets and positions are only added after the others, so a write-back copies
//! those it read (roll-lqjj.44). Who wrote the events a state read alone asks about, and where
//! machines' journals broke, are in the state part (roll-lqjj.35).

use std::collections::HashMap;
use std::ops::Range;

use indexmap::IndexSet;

use super::{Clock, Knowledge, Place, UNOBSERVED};
use crate::journal::Event;
use crate::projection::codec::{Reader, Unreadable, Writer};

impl Knowledge {
    /// The record of every event from the `from`th on, in the journal's order (roll-lqjj.44): its
    /// id, causes, machine and actor. Between compactions an event's record never changes and
    /// records are only added after it, so a write-back copies the records it read; and so its
    /// place and its clock, each written in a column of their own ([`Knowledge::write_places`],
    /// [`Knowledge::write_clocks`]), which a decode reads in a loop of its own.
    pub(crate) fn write_records(&self, w: &mut Writer, from: usize) {
        for (id, event) in self.events.iter().skip(from) {
            w.id(*id);
            w.ids(event.deps().iter());
            w.name(event.shared_replica());
            w.optional_shared(event.shared_actor());
        }
    }

    /// `n` records [`Knowledge::write_records`] wrote, after the events it has: still none placed.
    pub(crate) fn read_records(&mut self, r: &mut Reader<'_>, n: usize) -> Result<(), Unreadable> {
        // exactly: `reserve` gives the entries as much room as the hash table has, up to twice n
        self.events.reserve_exact(n);
        for _ in 0..n {
            let id = r.id()?;
            let deps = r.ids()?.into_boxed_slice();
            let event = Event::applied(id, deps, r.name()?, r.optional_shared()?);
            if self.events.insert(id, event).is_some() {
                return Err(Unreadable("an event twice"));
            }
        }
        Ok(())
    }

    /// The place of every event from the `from`th on.
    pub(crate) fn write_places(&self, w: &mut Writer, from: usize) {
        for place in self.ancestry.places.iter().skip(from) {
            let (kind, chain, position) = match place {
                Place::Nowhere => (0, 0, 0),
                Place::Sequenced(chain, position) => (1, *chain, *position),
                Place::Linked(chain, position) => (2, *chain, *position),
            };
            w.byte(kind);
            w.number(u64::from(chain));
            w.number(u64::from(position));
        }
    }

    /// `n` places [`Knowledge::write_places`] wrote, after those it has.
    pub(crate) fn read_places(&mut self, r: &mut Reader<'_>, n: usize) -> Result<(), Unreadable> {
        let places = &mut self.ancestry.places;
        places.reserve(n);
        for _ in 0..n {
            let (kind, chain, position) = (r.byte()?, r.small()?, r.small()?);
            places.push(match kind {
                0 => Place::Nowhere,
                1 => Place::Sequenced(chain, position),
                2 => Place::Linked(chain, position),
                _ => return Err(Unreadable("a place")),
            });
        }
        Ok(())
    }

    /// The clock of every event from the `from`th on, which refers to the chain sets and positions
    /// [`Knowledge::write_arena`] writes.
    pub(crate) fn write_clocks(&self, w: &mut Writer, from: usize) {
        for clock in self.ancestry.clocks.iter().skip(from) {
            w.byte(u8::from(*clock != UNOBSERVED));
            if *clock != UNOBSERVED {
                w.number(u64::from(clock.set));
                w.count(clock.start);
            }
        }
    }

    /// `n` clocks [`Knowledge::write_clocks`] wrote, after those it has.
    pub(crate) fn read_clocks(&mut self, r: &mut Reader<'_>, n: usize) -> Result<(), Unreadable> {
        let clocks = &mut self.ancestry.clocks;
        clocks.reserve(n);
        for _ in 0..n {
            clocks.push(if r.flag()? {
                Clock {
                    set: r.small()?,
                    start: r.index()?,
                }
            } else {
                UNOBSERVED
            });
        }
        Ok(())
    }

    /// What applying more events needs besides the records: the machines, each machine's
    /// applicable positions, and the chains of unsequenced events.
    pub(crate) fn write_chains(&self, w: &mut Writer) {
        w.count(self.replicas.len());
        for replica in &self.replicas {
            w.text(replica);
        }
        w.count(self.applicable.len());
        for positions in &self.applicable {
            w.count(positions.len());
            for position in positions {
                w.number(u64::from(*position));
            }
        }
        let ancestry = &self.ancestry;
        w.number(u64::from(ancestry.sequenced));
        w.count(ancestry.lengths.len());
        for length in &ancestry.lengths {
            w.number(u64::from(*length));
        }
        let mut last: Vec<(&String, &u32)> = ancestry.last.iter().collect();
        last.sort();
        w.count(last.len());
        for (replica, chain) in last {
            w.text(replica);
            w.number(u64::from(*chain));
        }
    }

    /// What [`Knowledge::write_chains`] wrote.
    pub(crate) fn read_chains(&mut self, r: &mut Reader<'_>) -> Result<(), Unreadable> {
        self.replicas = (0..r.count()?)
            .map(|_| r.text())
            .collect::<Result<_, _>>()?;
        let mut applicable = Vec::new();
        for _ in 0..r.count()? {
            let positions = (0..r.count()?)
                .map(|_| r.small())
                .collect::<Result<_, _>>()?;
            applicable.push(positions);
        }
        self.applicable = applicable;
        let ancestry = &mut self.ancestry;
        ancestry.sequenced = r.small()?;
        ancestry.lengths = (0..r.count()?)
            .map(|_| r.small())
            .collect::<Result<_, _>>()?;
        let mut last = HashMap::new();
        for _ in 0..r.count()? {
            let replica = r.text()?;
            last.insert(replica, r.small()?);
        }
        ancestry.last = last;
        Ok(())
    }

    /// How many chain sets and positions the ancestry's clocks share: only ever added to between
    /// compactions.
    pub(crate) fn arena(&self) -> (usize, usize) {
        (self.ancestry.sets.len(), self.ancestry.positions.len())
    }

    /// The chain sets from the `sets`th on and the positions from the `positions`th on, which
    /// clocks refer to: what follows `stored`, the bytes of those before, copied as they were read
    /// ([`Knowledge::read_arena`]), when they are given.
    pub(crate) fn write_arena(&self, w: &mut Writer, stored: Option<Arena<'_>>) {
        let ancestry = &self.ancestry;
        let (sets, positions) = stored.map_or((0, 0), |arena| arena.counts);
        w.count(ancestry.sets.len());
        if let Some(arena) = stored {
            w.copy(arena.sets);
        }
        for set in ancestry.sets.iter().skip(sets) {
            w.count(set.len());
            for chain in set.iter() {
                w.number(u64::from(*chain));
            }
        }
        w.count(ancestry.positions.len());
        if let Some(arena) = stored {
            w.copy(arena.positions);
        }
        for position in ancestry.positions.iter().skip(positions) {
            w.number(u64::from(*position));
        }
    }

    /// What [`Knowledge::write_arena`] wrote, for the records [`Knowledge::read_records`] read; and
    /// where in `r`'s bytes the sets and the positions are, to be copied.
    pub(crate) fn read_arena(
        &mut self,
        r: &mut Reader<'_>,
    ) -> Result<(Range<usize>, Range<usize>), Unreadable> {
        let n = r.count()?;
        let from = r.at();
        let mut sets = IndexSet::with_capacity(n);
        for _ in 0..n {
            let set: Box<[u32]> = (0..r.count()?)
                .map(|_| r.small())
                .collect::<Result<_, _>>()?;
            if !sets.insert(set) {
                return Err(Unreadable("a chain set twice"));
            }
        }
        let set_bytes = from..r.at();
        let n = r.count()?;
        let from = r.at();
        let positions = (0..n).map(|_| r.small()).collect::<Result<_, _>>()?;
        let position_bytes = from..r.at();
        let ancestry = &mut self.ancestry;
        ancestry.sets = sets;
        ancestry.positions = positions;
        let events = self.events.len();
        if ancestry.places.len() != events || ancestry.clocks.len() != events {
            return Err(Unreadable("places"));
        }
        Ok((set_bytes, position_bytes))
    }
}

/// The chain sets and positions a write-back read, as bytes to copy, and how many of each.
#[derive(Clone, Copy)]
pub(crate) struct Arena<'a> {
    pub(crate) counts: (usize, usize),
    pub(crate) sets: &'a [u8],
    pub(crate) positions: &'a [u8],
}
