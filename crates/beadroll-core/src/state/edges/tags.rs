//! The additions behind an issue's edges ([`DepTags`]), held so that finding an edge, or adding
//! one, does not read every edge the issue has (roll-lqjj.21). Most issues have a few edges, which
//! a vector holds and a lookup compares in order, as `state::small` holds an issue's registers and
//! tags (roll-dih8.11). An issue with more than [`LISTED`] has them indexed besides: a table of
//! each edge's place in the vector, hashed by the edge's target through the vector, so that no edge
//! is held twice. An edge is then found or added by looking at the edges on its target alone, where
//! comparing it with every edge before it made an issue given N edges cost N², whether an import
//! named them at once, the keys it waited for came one at a time, or `dep add` added them. The
//! vector stays what every reader iterates, where it always was: the edges keep the order they were
//! first added in, and an edge removed keeps its place, its additions emptied.

use std::fmt;
use std::hash::{BuildHasher, RandomState};

use hashbrown::HashTable;

use crate::work::{Work, tally};

use super::super::small::{IdMap, room_for_one};
use super::{Edge, Meta, Target};

/// How many edges an issue holds before they are indexed: a few compared in order are found sooner
/// than one hashed, and nearly every issue has fewer. In this crate's own tests it is one, so that
/// the differential histories, whose issues have a few edges each, take the indexed path wherever
/// an issue is changed again once it has two, and are checked there against what the edges
/// answered as a vector searched in order (`projection::tests::edges`); the other suites build
/// this crate without `cfg(test)`, and keep the bound bead runs with.
pub(crate) const LISTED: usize = if cfg!(test) { 1 } else { 16 };

/// How many changes search the edges in order after they are read back before one builds their
/// index: about what building it costs, since it reads every edge once, as a search does, and
/// hashes and places each, which costs as much as comparing six to twenty, a text target nearer
/// six and an id nearer twenty (measured in the review of roll-lqjj.21). A command reads a
/// projection back and changes an issue once or a few times, so most never build one, and a merge
/// that changes an issue many times pays at most about three times what knowing in advance would
/// have, about two and a half when its targets are issues. In this crate's own tests it is one, so
/// that a history read back takes the indexed path at its second change.
const SEARCHES: u8 = if cfg!(test) { 1 } else { 12 };

/// An edge and the additions behind it.
type Entry = (Edge, IdMap<Meta>);

/// The additions behind each of an issue's edges, by the event that added it, in the order the
/// edges were first added: empty once an edge is removed. Found by target and kind without reading
/// the others once the issue has many (roll-lqjj.21).
#[derive(Clone, Default)]
pub struct DepTags {
    /// Every edge the issue has had, in the order first added: what every reader iterates, held
    /// in place rather than with the index, since every pass over the whole state's edges (the
    /// parent-child index, readiness, dependents) reads each issue's, and reached them measurably
    /// slower behind a choice between two forms.
    entries: Vec<Entry>,
    /// Their index: none while there are at most [`LISTED`], a pointer for nearly every issue.
    /// Past that a change builds it, unless the edges were read back with that many, which hold
    /// one not built yet ([`Index::built`]) that counts the changes searching them in order until
    /// one more than [`SEARCHES`] builds it.
    index: Option<Box<Index>>,
}

/// The places of an issue's edges, hashed by target.
#[derive(Clone)]
struct Index {
    /// The place of every edge, hashed by its target: one table finds an edge by its target and
    /// kind, and every edge on a target, which a removal of any kind asks for. Empty in an index
    /// not built yet.
    places: HashTable<usize>,
    /// What hashes a target, its keys drawn for each index, so that no targets chosen in advance
    /// make an issue's edges collide.
    hasher: RandomState,
    /// In an index not built yet, how many changes have searched the edges in order since they
    /// were read back.
    searched: u8,
}

impl DepTags {
    /// The edges of `entries`, in their order, as a projection reads them back: indexed at the
    /// change after [`SEARCHES`] when they are more than [`LISTED`], so that a state only read, or
    /// changed a few times, builds no index.
    pub(crate) fn from_entries(entries: Vec<Entry>) -> DepTags {
        let index = (entries.len() > LISTED).then(|| Box::new(Index::unbuilt()));
        DepTags { entries, index }
    }

    /// How many edges the issue has had, removed ones included.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it has had none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether its edges are indexed: from a change once there are more than 16, and from the
    /// thirteenth once they are read back from a projection with that many.
    pub fn indexed(&self) -> bool {
        self.built().is_some()
    }

    /// Each edge and the additions behind it, in the order the edges were first added.
    pub fn iter(&self) -> <&DepTags as IntoIterator>::IntoIter {
        self.into_iter()
    }

    /// Each edge, in order.
    pub fn keys(&self) -> impl Iterator<Item = &Edge> {
        self.entries.iter().map(|(edge, _)| edge)
    }

    /// The additions behind each edge, in order.
    pub fn values(&self) -> impl Iterator<Item = &IdMap<Meta>> {
        self.entries.iter().map(|(_, tags)| tags)
    }

    /// The same, to change; no edge is added or moved through it.
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut IdMap<Meta>> {
        self.entries.iter_mut().map(|(_, tags)| tags)
    }

    /// The additions behind the edge on `to` of `kind`, if the issue has had it: found by its
    /// target once the edges are indexed, else by comparing them in order.
    pub fn get(&self, to: &Target, kind: &str) -> Option<&IdMap<Meta>> {
        let at = match self.built() {
            Some(index) => {
                let hash = index.hash(to);
                index.first(&self.entries, hash, to, |edge| *edge.kind == *kind)
            }
            None => self.entries.iter().position(|(edge, _)| {
                tally(Work::Probe, 1);
                edge.is(to, kind)
            }),
        };
        self.entries.get(at?).map(|(_, tags)| tags)
    }

    /// The additions behind `edge`, to change: none, at the end, when the issue has never had it.
    pub(crate) fn added(&mut self, edge: Edge) -> &mut IdMap<Meta> {
        self.ready_to_change();
        let hash = self.built().map(|index| index.hash(&edge.to));
        let found = match (self.built(), hash) {
            (Some(index), Some(hash)) => {
                index.first(&self.entries, hash, &edge.to, |was| was.kind == edge.kind)
            }
            _ => self.entries.iter().position(|(was, _)| {
                tally(Work::Probe, 1);
                *was == edge
            }),
        };
        let at = match found {
            Some(at) => at,
            None => {
                let at = self.entries.len();
                room_for_one(&mut self.entries);
                self.entries.push((edge, IdMap::default()));
                // a hash only of an index built
                if let (Some(index), Some(hash)) = (self.index.as_deref_mut(), hash) {
                    index.place(&self.entries, hash, at);
                }
                at
            }
        };
        // a place just found, or the entry just made: in bounds
        &mut self.entries[at].1
    }

    /// Hands `change` each edge on `to` and the additions behind it, to change, in the order the
    /// edges were first added.
    pub(crate) fn for_each_to(
        &mut self,
        to: &Target,
        mut change: impl FnMut(&Edge, &mut IdMap<Meta>),
    ) {
        self.ready_to_change();
        match self.built() {
            Some(index) => {
                for at in index.on(&self.entries, to) {
                    if let Some((edge, tags)) = self.entries.get_mut(at) {
                        change(edge, tags);
                    }
                }
            }
            None => {
                for (edge, tags) in &mut self.entries {
                    tally(Work::Probe, 1);
                    if edge.to == *to {
                        change(edge, tags);
                    }
                }
            }
        }
    }

    /// The index, when it is built.
    fn built(&self) -> Option<&Index> {
        self.index.as_deref().filter(|index| index.built())
    }

    /// Readies the edges for a change: builds their index when they are more than [`LISTED`] and
    /// have none, or have one not built yet that [`SEARCHES`] changes have searched in order, and
    /// otherwise counts this change among those.
    fn ready_to_change(&mut self) {
        let build = match self.index.as_deref_mut() {
            Some(index) if index.built() => false,
            Some(index) if index.searched < SEARCHES => {
                index.searched += 1;
                false
            }
            Some(_) => true,
            None => self.entries.len() > LISTED,
        };
        if build {
            self.index = Some(Box::new(Index::of(&self.entries)));
        }
    }
}

impl Index {
    /// An index not built yet, which edges read back hold until a change builds it.
    fn unbuilt() -> Index {
        Index {
            places: HashTable::new(),
            hasher: RandomState::new(),
            searched: 0,
        }
    }

    /// The index of `entries`: every place, an edge listed twice too, which only bytes bead did not
    /// write can hold, and which a lookup finds where a search in order finds it, at its first; and
    /// room for one more, the edge the change that builds it may add.
    fn of(entries: &[Entry]) -> Index {
        tally(Work::Probe, entries.len());
        let mut index = Index {
            places: HashTable::with_capacity(entries.len() + 1),
            hasher: RandomState::new(),
            searched: 0,
        };
        for (at, (edge, _)) in entries.iter().enumerate() {
            let hash = index.hash(&edge.to);
            index.place(entries, hash, at);
        }
        index
    }

    /// Whether it is built: an index of edges, which are more than [`LISTED`], holds a place.
    fn built(&self) -> bool {
        !self.places.is_empty()
    }

    /// What `to` hashes to.
    fn hash(&self, to: &Target) -> u64 {
        self.hasher.hash_one(to)
    }

    /// Indexes place `at` of `entries`, whose target hashes to `hash`.
    fn place(&mut self, entries: &[Entry], hash: u64, at: usize) {
        let Index { places, hasher, .. } = self;
        places.insert_unique(hash, at, |&place| hash_at(hasher, entries, place));
    }

    /// The places of the edges of `entries` on `to`, in order.
    fn on(&self, entries: &[Entry], to: &Target) -> Vec<usize> {
        tally(Work::Probe, 1);
        let mut places: Vec<usize> = (self.places.iter_hash(self.hash(to)))
            .copied()
            .filter(|&at| {
                tally(Work::Probe, 1);
                entries.get(at).is_some_and(|(edge, _)| edge.to == *to)
            })
            .collect();
        places.sort_unstable();
        places
    }

    /// The first place of an edge of `entries` on `to`, which hashes to `hash`, that `wanted` says
    /// is the one.
    fn first(
        &self,
        entries: &[Entry],
        hash: u64,
        to: &Target,
        mut wanted: impl FnMut(&Edge) -> bool,
    ) -> Option<usize> {
        tally(Work::Probe, 1);
        (self.places.iter_hash(hash))
            .copied()
            .filter(|&at| {
                tally(Work::Probe, 1);
                (entries.get(at)).is_some_and(|(edge, _)| edge.to == *to && wanted(edge))
            })
            .min()
    }
}

/// The hash of the target of the edge at `place` of `entries`.
fn hash_at(hasher: &RandomState, entries: &[Entry], place: usize) -> u64 {
    entries
        .get(place)
        .map_or(0, |(edge, _)| hasher.hash_one(&edge.to))
}

impl PartialEq for DepTags {
    /// The same edges and additions in the same order, whether indexed or not.
    fn eq(&self, other: &DepTags) -> bool {
        self.entries == other.entries
    }
}

impl fmt::Debug for DepTags {
    /// Its edges and the additions behind them, in order, whether indexed or not.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DepTags").field(&self.entries).finish()
    }
}

impl<'a> IntoIterator for &'a DepTags {
    type Item = (&'a Edge, &'a IdMap<Meta>);
    type IntoIter = std::iter::Map<std::slice::Iter<'a, Entry>, fn(&'a Entry) -> Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().map(|(edge, tags)| (edge, tags))
    }
}

#[cfg(test)]
mod tests;
