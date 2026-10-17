//! The small maps and sets an issue keeps (roll-dih8.11): registers, tag sets and dependencies.
//! Each holds a few entries, mostly one, where a `BTreeMap` takes a node of eleven and an
//! `IndexMap` a hash table, so they are vectors, searched in order: [`VecMap`] keeps the order
//! entries were first made in, as the `IndexMap`s did; [`IdMap`] and [`IdSet`] keep event ids in
//! order, as the `BTreeMap`s and `BTreeSet`s did.

use crate::id::EventId;

/// How long a vector grows by one entry at a time, before it doubles as vectors do: an issue's
/// registers and tags mostly hold one entry, where a vector's first growth makes room for four.
const EXACT: usize = 8;

/// Room for one more entry in `entries`: exactly one while it is short.
pub(crate) fn room_for_one<T>(entries: &mut Vec<T>) {
    if entries.len() == entries.capacity() && entries.len() < EXACT {
        entries.reserve_exact(1);
    }
}

/// A map that keeps its entries in the order each key was first inserted.
#[derive(Clone, Debug, PartialEq)]
pub struct VecMap<K, V>(Vec<(K, V)>);

impl<K, V> Default for VecMap<K, V> {
    fn default() -> VecMap<K, V> {
        VecMap(Vec::new())
    }
}

impl<K: PartialEq, V> VecMap<K, V> {
    /// The value of `key`.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Whether it has `key`.
    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// The value of `key`, to change.
    pub(crate) fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// `value` under `key`: in `key`'s place when it has one, else at the end.
    pub(crate) fn insert(&mut self, key: K, value: V) {
        match self.get_mut(&key) {
            Some(slot) => *slot = value,
            None => {
                room_for_one(&mut self.0);
                self.0.push((key, value));
            }
        }
    }

    /// The value of `key`, made with `make` at the end when it has none.
    pub(crate) fn get_or_insert_with(&mut self, key: K, make: impl FnOnce() -> V) -> &mut V {
        let at = match self.0.iter().position(|(k, _)| *k == key) {
            Some(at) => at,
            None => {
                room_for_one(&mut self.0);
                self.0.push((key, make()));
                self.0.len() - 1
            }
        };
        // a position just found, or the entry just made: in bounds
        &mut self.0[at].1
    }
}

impl<K, V> VecMap<K, V> {
    /// The map of `entries`, whose keys are distinct, in their order.
    pub(crate) fn from_distinct(entries: Vec<(K, V)>) -> VecMap<K, V> {
        VecMap(entries)
    }

    /// Its entries, in their order.
    pub(crate) fn into_entries(self) -> Vec<(K, V)> {
        self.0
    }

    /// How many entries it has.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Its entries, in order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.0.iter().map(|(k, v)| (k, v))
    }

    /// Its keys, in order.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.0.iter().map(|(k, _)| k)
    }

    /// Its values, in order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.0.iter().map(|(_, v)| v)
    }

    /// Its entries, in order, the values to change.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.0.iter_mut().map(|(k, v)| (&*k, v))
    }

    /// Its values, in order, to change.
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.0.iter_mut().map(|(_, v)| v)
    }
}

impl<K: PartialEq, V> FromIterator<(K, V)> for VecMap<K, V> {
    /// Entries in the order their keys come first, each the last value its key is given.
    fn from_iter<I: IntoIterator<Item = (K, V)>>(entries: I) -> VecMap<K, V> {
        let mut map = VecMap::default();
        for (key, value) in entries {
            map.insert(key, value);
        }
        map
    }
}

impl<'a, K, V> IntoIterator for &'a VecMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = std::iter::Map<std::slice::Iter<'a, (K, V)>, fn(&'a (K, V)) -> (&'a K, &'a V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|(k, v)| (k, v))
    }
}

/// A map from event ids, its entries in the order of their ids.
#[derive(Clone, Debug, PartialEq)]
pub struct IdMap<V>(Vec<(EventId, V)>);

impl<V> Default for IdMap<V> {
    fn default() -> IdMap<V> {
        IdMap(Vec::new())
    }
}

impl<V> IdMap<V> {
    /// The map of `entries`, when their ids ascend.
    pub(crate) fn from_ascending(entries: Vec<(EventId, V)>) -> Option<IdMap<V>> {
        let ascend = entries
            .windows(2)
            .all(|pair| matches!(pair, [a, b] if a.0 < b.0));
        ascend.then_some(IdMap(entries))
    }

    /// The map of `value` under `id` alone.
    pub(crate) fn of(id: EventId, value: V) -> IdMap<V> {
        IdMap(vec![(id, value)])
    }

    /// How many entries it has.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Its entries, by id.
    pub fn iter(&self) -> impl Iterator<Item = (&EventId, &V)> {
        self.0.iter().map(|(k, v)| (k, v))
    }

    /// Its values, by id.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.0.iter().map(|(_, v)| v)
    }

    /// Its values, by id, to change.
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.0.iter_mut().map(|(_, v)| v)
    }

    /// Whether it has an entry for `id`.
    pub fn contains_key(&self, id: &EventId) -> bool {
        self.0.binary_search_by_key(id, |(k, _)| *k).is_ok()
    }

    /// The entry of the smallest id.
    pub fn first(&self) -> Option<(&EventId, &V)> {
        self.0.first().map(|(k, v)| (k, v))
    }

    /// The entry of the largest id.
    pub fn last(&self) -> Option<(&EventId, &V)> {
        self.0.last().map(|(k, v)| (k, v))
    }

    /// `value` under `id`, in its place; a value `id` had is replaced.
    pub(crate) fn insert(&mut self, id: EventId, value: V) {
        match self.0.binary_search_by_key(&id, |(k, _)| *k) {
            Ok(at) => {
                if let Some(slot) = self.0.get_mut(at) {
                    slot.1 = value;
                }
            }
            Err(at) => {
                room_for_one(&mut self.0);
                self.0.insert(at, (id, value));
            }
        }
    }

    /// `value` under `id`, in its place, unless `id` has a value.
    pub(crate) fn insert_if_absent(&mut self, id: EventId, value: V) {
        if let Err(at) = self.0.binary_search_by_key(&id, |(k, _)| *k) {
            room_for_one(&mut self.0);
            self.0.insert(at, (id, value));
        }
    }

    /// Takes away `id`'s entry.
    pub(crate) fn remove(&mut self, id: &EventId) {
        if let Ok(at) = self.0.binary_search_by_key(id, |(k, _)| *k) {
            self.0.remove(at);
        }
    }

    /// Keeps the entries `keep` says to.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&EventId, &V) -> bool) {
        self.0.retain(|(k, v)| keep(k, v));
    }
}

impl<'a, V> IntoIterator for &'a IdMap<V> {
    type Item = (&'a EventId, &'a V);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (EventId, V)>,
        fn(&'a (EventId, V)) -> (&'a EventId, &'a V),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|(k, v)| (k, v))
    }
}

/// A set of event ids, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IdSet(Vec<EventId>);

impl IdSet {
    /// The set of `ids`, when they ascend.
    pub(crate) fn from_ascending(ids: Vec<EventId>) -> Option<IdSet> {
        let ascend = ids.windows(2).all(|pair| matches!(pair, [a, b] if a < b));
        ascend.then_some(IdSet(ids))
    }

    /// The set of `id` alone.
    pub(crate) fn of(id: EventId) -> IdSet {
        IdSet(vec![id])
    }

    /// How many ids it has.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether it has `id`.
    pub fn contains(&self, id: &EventId) -> bool {
        self.0.binary_search(id).is_ok()
    }

    /// Its ids, in order.
    pub fn iter(&self) -> std::slice::Iter<'_, EventId> {
        self.0.iter()
    }

    /// Adds `id`.
    pub(crate) fn insert(&mut self, id: EventId) {
        if let Err(at) = self.0.binary_search(&id) {
            room_for_one(&mut self.0);
            self.0.insert(at, id);
        }
    }

    /// Keeps the ids `keep` says to.
    pub(crate) fn retain(&mut self, keep: impl FnMut(&EventId) -> bool) {
        self.0.retain(keep);
    }
}

impl<'a> IntoIterator for &'a IdSet {
    type Item = &'a EventId;
    type IntoIter = std::slice::Iter<'a, EventId>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> EventId {
        EventId::parse(&format!("01900000-0000-7000-8000-{n:012x}")).unwrap()
    }

    #[test]
    fn a_vec_map_keeps_the_order_keys_were_first_made_in() {
        let mut map: VecMap<&str, u32> = VecMap::default();
        *map.get_or_insert_with("b", || 1) += 1;
        *map.get_or_insert_with("a", || 5) += 1;
        *map.get_or_insert_with("b", || 9) += 1;
        assert_eq!(map.iter().collect::<Vec<_>>(), [(&"b", &3), (&"a", &6)]);
        assert!(map.contains_key(&"a") && !map.contains_key(&"c"));
    }

    #[test]
    fn id_maps_and_sets_keep_ids_in_order() {
        let mut map = IdMap::default();
        for n in [5, 1, 3, 1] {
            map.insert(id(n), n * 10);
        }
        let entries: Vec<(EventId, u64)> = map.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(entries, [(id(1), 10), (id(3), 30), (id(5), 50)]);
        assert_eq!(map.first().map(|(k, _)| *k), Some(id(1)));
        assert_eq!(map.last().map(|(_, v)| *v), Some(50));
        map.remove(&id(3));
        map.retain(|k, _| *k != id(5));
        assert_eq!(map.len(), 1);
        let mut set = IdSet::of(id(4));
        for n in [2, 4, 9] {
            set.insert(id(n));
        }
        assert_eq!(
            set.iter().copied().collect::<Vec<_>>(),
            [id(2), id(4), id(9)]
        );
        assert!(set.contains(&id(9)) && !set.contains(&id(3)));
        set.retain(|k| *k == id(2));
        assert_eq!(set.len(), 1);
    }
}
