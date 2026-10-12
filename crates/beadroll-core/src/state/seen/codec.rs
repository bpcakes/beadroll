//! What a projection keeps of what every event had seen, as bytes ([`crate::projection`]): the
//! events, each with its causes, machine and actor; the chains; each event's place and clock.

use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};

use crate::journal::Event;
use crate::projection::codec::{Reader, Unreadable, Writer};

use super::{Ancestry, Clock, Knowledge, Place, UNOBSERVED};

impl Knowledge {
    pub(crate) fn write_to(&self, w: &mut Writer) {
        w.count(self.events.len());
        for (id, event) in &self.events {
            w.id(*id);
            w.ids(event.deps());
            w.name(event.shared_replica());
            w.optional_shared(event.shared_actor());
        }
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
        self.ancestry.write_to(w);
        w.count(self.chain_breaks.len());
        for (replica, positions) in &self.chain_breaks {
            w.text(replica);
            w.count(positions.len());
            for position in positions {
                w.count(*position);
            }
        }
    }

    pub(crate) fn read_from(r: &mut Reader<'_>) -> Result<Knowledge, Unreadable> {
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
        let mut applicable = Vec::new();
        for _ in 0..r.count()? {
            let positions = (0..r.count()?)
                .map(|_| r.small())
                .collect::<Result<_, _>>()?;
            applicable.push(positions);
        }
        let ancestry = Ancestry::read_from(r)?;
        if ancestry.places.len() != events.len() || ancestry.clocks.len() != events.len() {
            return Err(Unreadable("places"));
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
            replicas,
            applicable,
            ancestry,
            chain_breaks,
        })
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
