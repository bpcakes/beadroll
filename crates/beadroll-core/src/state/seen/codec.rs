//! What a projection keeps of what every event had seen, as bytes ([`crate::projection`]), in three
//! parts: what a state answers with alone, the writers of the events behind a register two writers
//! set without seeing each other ([`crate::state::State::who`], its conflicts' writers) and where
//! machines' journals broke, which its state part holds (roll-lqjj.35); and in its index, which
//! only applying more events reads, every event it names with its causes, machine and actor, the
//! machines, and each machine's applicable positions and each event's place and clock.

use std::collections::{HashMap, HashSet};

use indexmap::{IndexMap, IndexSet};

use crate::id::EventId;
use crate::journal::Event;
use crate::projection::codec::{Reader, Unreadable, Writer};

use super::{Ancestry, Clock, Knowledge, Place, UNOBSERVED};

impl Knowledge {
    /// What a state answers with alone: the machine and actor of the events among `asked` (in the
    /// journal's order), and where machines' journals broke.
    pub(crate) fn write_writers(&self, w: &mut Writer, asked: &HashSet<EventId>) {
        let writers: Vec<(&EventId, &Event)> = (self.events.iter())
            .filter(|(id, _)| asked.contains(*id))
            .collect();
        w.count(writers.len());
        for (id, event) in writers {
            w.id(*id);
            w.name(event.shared_replica());
            w.optional_shared(event.shared_actor());
        }
        w.count(self.chain_breaks.len());
        for (replica, positions) in &self.chain_breaks {
            w.text(replica);
            w.count(positions.len());
            for position in positions {
                w.count(*position);
            }
        }
    }

    /// What [`Knowledge::write_writers`] wrote: a knowledge that has placed no event yet, whose
    /// events are the ones a state asks who wrote, without their causes.
    pub(crate) fn read_writers(r: &mut Reader<'_>) -> Result<Knowledge, Unreadable> {
        let n = r.count()?;
        let mut events = IndexMap::with_capacity(n);
        for _ in 0..n {
            let id = r.id()?;
            let event = Event::applied(id, Box::default(), r.name()?, r.optional_shared()?);
            events.insert(id, event);
        }
        let mut chain_breaks = IndexMap::new();
        for _ in 0..r.count()? {
            let replica = r.text()?;
            let positions = (0..r.count()?)
                .map(|_| r.index())
                .collect::<Result<_, _>>()?;
            chain_breaks.insert(replica, positions);
        }
        Ok(Knowledge {
            events,
            replicas: Vec::new(),
            applicable: Vec::new(),
            ancestry: Ancestry::default(),
            chain_breaks,
        })
    }

    /// What applying more events needs of the events: each with its causes, machine and actor, and
    /// the machines.
    pub(crate) fn write_events(&self, w: &mut Writer) {
        w.count(self.events.len());
        for (id, event) in &self.events {
            w.id(*id);
            w.ids(event.deps().iter());
            w.name(event.shared_replica());
            w.optional_shared(event.shared_actor());
        }
        w.count(self.replicas.len());
        for replica in &self.replicas {
            w.text(replica);
        }
    }

    /// What [`Knowledge::write_events`] wrote, in place of the events [`Knowledge::read_writers`]
    /// read: still none placed.
    pub(crate) fn read_events(&mut self, r: &mut Reader<'_>) -> Result<(), Unreadable> {
        let n = r.count()?;
        let mut events = IndexMap::with_capacity(n);
        for _ in 0..n {
            let id = r.id()?;
            let deps = r.ids()?.into_boxed_slice();
            let event = Event::applied(id, deps, r.name()?, r.optional_shared()?);
            events.insert(id, event);
        }
        let replicas = (0..r.count()?)
            .map(|_| r.text())
            .collect::<Result<_, _>>()?;
        self.events = events;
        self.replicas = replicas;
        Ok(())
    }

    /// What applying more events needs: each machine's applicable positions, each event's place
    /// and clock.
    pub(crate) fn write_index(&self, w: &mut Writer) {
        w.count(self.applicable.len());
        for positions in &self.applicable {
            w.count(positions.len());
            for position in positions {
                w.number(u64::from(*position));
            }
        }
        self.ancestry.write_to(w);
    }

    /// What [`Knowledge::write_index`] wrote, for the events [`Knowledge::read_events`] read.
    pub(crate) fn read_index(&mut self, r: &mut Reader<'_>) -> Result<(), Unreadable> {
        let mut applicable = Vec::new();
        for _ in 0..r.count()? {
            let positions = (0..r.count()?)
                .map(|_| r.small())
                .collect::<Result<_, _>>()?;
            applicable.push(positions);
        }
        let ancestry = Ancestry::read_from(r)?;
        let n = self.events.len();
        if ancestry.places.len() != n || ancestry.clocks.len() != n {
            return Err(Unreadable("places"));
        }
        self.applicable = applicable;
        self.ancestry = ancestry;
        Ok(())
    }
}

impl Ancestry {
    fn write_to(&self, w: &mut Writer) {
        w.count(self.places.len());
        for place in &self.places {
            let (kind, chain, position) = match place {
                Place::Nowhere => (0, 0, 0),
                Place::Sequenced(chain, position) => (1, *chain, *position),
                Place::Linked(chain, position) => (2, *chain, *position),
            };
            w.byte(kind);
            w.number(u64::from(chain));
            w.number(u64::from(position));
        }
        w.count(self.clocks.len());
        for clock in &self.clocks {
            w.flag(*clock != UNOBSERVED);
            if *clock != UNOBSERVED {
                w.number(u64::from(clock.set));
                w.count(clock.start);
            }
        }
        w.count(self.sets.len());
        for set in &self.sets {
            w.count(set.len());
            for chain in set.iter() {
                w.number(u64::from(*chain));
            }
        }
        w.count(self.positions.len());
        for position in &self.positions {
            w.number(u64::from(*position));
        }
        w.number(u64::from(self.sequenced));
        w.count(self.lengths.len());
        for length in &self.lengths {
            w.number(u64::from(*length));
        }
        let mut last: Vec<(&String, &u32)> = self.last.iter().collect();
        last.sort();
        w.count(last.len());
        for (replica, chain) in last {
            w.text(replica);
            w.number(u64::from(*chain));
        }
    }

    fn read_from(r: &mut Reader<'_>) -> Result<Ancestry, Unreadable> {
        let mut places = Vec::new();
        for _ in 0..r.count()? {
            let (kind, chain, position) = (r.byte()?, r.small()?, r.small()?);
            places.push(match kind {
                0 => Place::Nowhere,
                1 => Place::Sequenced(chain, position),
                2 => Place::Linked(chain, position),
                _ => return Err(Unreadable("a place")),
            });
        }
        let mut clocks = Vec::new();
        for _ in 0..r.count()? {
            clocks.push(if r.flag()? {
                Clock {
                    set: r.small()?,
                    start: r.index()?,
                }
            } else {
                UNOBSERVED
            });
        }
        let mut sets = IndexSet::new();
        for _ in 0..r.count()? {
            let set: Box<[u32]> = (0..r.count()?)
                .map(|_| r.small())
                .collect::<Result<_, _>>()?;
            sets.insert(set);
        }
        let positions = (0..r.count()?)
            .map(|_| r.small())
            .collect::<Result<_, _>>()?;
        let sequenced = r.small()?;
        let lengths = (0..r.count()?)
            .map(|_| r.small())
            .collect::<Result<_, _>>()?;
        let mut last = HashMap::new();
        for _ in 0..r.count()? {
            let replica = r.text()?;
            last.insert(replica, r.small()?);
        }
        Ok(Ancestry {
            places,
            clocks,
            sets,
            positions,
            sequenced,
            lengths,
            last,
        })
    }
}
