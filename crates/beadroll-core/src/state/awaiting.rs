//! Where the imported dependencies that wait for a key are (roll-lqjj.18): indexed by the key each
//! names, so that a create visits the entries its key settles and no others.

use std::collections::BTreeSet;

use indexmap::IndexMap;

use crate::id::EventId;
use crate::json::Value;

use super::Issue;

/// Every imported dependency still waiting, as the key it names, its issue and its place in that
/// issue's list ([`Issue::dangling`]). One that names its own issue's key waits for good, and is
/// listed too, so that a removal by key finds it. An entry taken out while events apply leaves null
/// in its place, so that the places of the others hold however many are taken from one list;
/// [`Awaiting::settle`] makes each such list whole again, in its order, once the events have
/// applied, and moves the entries after the first one taken to the places they then have. Ordered,
/// so that what one key settles is one range and nothing a hash would order reaches the state.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Awaiting {
    /// Key, issue and place.
    entries: BTreeSet<(String, EventId, usize)>,
    /// The issues an entry was taken from since the last [`Awaiting::settle`], as often as one was.
    unsettled: Vec<EventId>,
}

/// The key an imported dependency names: `None` for an entry taken out, which is null.
fn named(dep: &Value) -> Option<&str> {
    dep.get("depends_on_id").and_then(Value::as_str)
}

/// Whether the entry was taken out, and only holds its place.
fn taken(dep: &Value) -> bool {
    matches!(dep, Value::Null)
}

impl Awaiting {
    /// Where every entry of `issues`' lists is, each list whole: what a state read back starts with.
    pub(crate) fn of<'a>(issues: impl Iterator<Item = &'a Issue>) -> Awaiting {
        let mut awaiting = Awaiting::default();
        for issue in issues {
            awaiting.wait(issue);
        }
        awaiting
    }

    /// Lists the entries of `issue`, its list whole, as waiting.
    pub(crate) fn wait(&mut self, issue: &Issue) {
        for (at, dep) in issue.dangling.iter().enumerate() {
            if let Some(key) = named(dep) {
                self.entries.insert((key.to_owned(), issue.uid, at));
            }
        }
    }

    /// Every entry that waits for `key`, no longer listed: each issue's, in the order of its list.
    pub(crate) fn arrived(&mut self, key: &str) -> Vec<(EventId, usize)> {
        if self.entries.is_empty() {
            return Vec::new(); // most trackers: nothing an import named is missing
        }
        let range = (key.to_owned(), EventId::from_u128(0), 0)
            ..=(key.to_owned(), EventId::from_u128(u128::MAX), usize::MAX);
        (self.entries.extract_if(range, |_| true))
            .map(|(_, uid, at)| (uid, at))
            .collect()
    }

    /// Takes out of `issue`'s list the entries that wait for `key` and that `chosen` picks.
    pub(crate) fn retract(
        &mut self,
        issue: &mut Issue,
        key: &str,
        chosen: impl Fn(&Value) -> bool,
    ) {
        let range = (key.to_owned(), issue.uid, 0)..=(key.to_owned(), issue.uid, usize::MAX);
        let retracted: Vec<usize> = (self.entries.extract_if(range, |(_, _, at)| {
            issue.dangling.get(*at).is_some_and(&chosen)
        }))
        .map(|(_, _, at)| at)
        .collect();
        for at in retracted {
            self.take(issue, at);
        }
    }

    /// The entry at `at` of `issue`'s list, which is no longer listed, taken out of the list: null in
    /// its place until the list is settled. `None` when there is none there.
    pub(crate) fn take(&mut self, issue: &mut Issue, at: usize) -> Option<Value> {
        let slot = issue.dangling.get_mut(at).filter(|dep| !taken(dep))?;
        self.unsettled.push(issue.uid);
        Some(std::mem::replace(slot, Value::Null))
    }

    /// Makes whole every list an entry was taken from, in its order, and lists each entry left at
    /// the place it then has: what a state is finished with, so that no null is ever seen.
    pub(crate) fn settle(&mut self, issues: &mut IndexMap<EventId, Box<Issue>>) {
        let mut unsettled = std::mem::take(&mut self.unsettled);
        unsettled.sort_unstable();
        unsettled.dedup();
        for uid in unsettled {
            let Some(issue) = issues.get_mut(&uid) else {
                continue;
            };
            let mut moved = Vec::new();
            let mut place = 0;
            for (at, dep) in issue.dangling.iter().enumerate() {
                if taken(dep) {
                    continue;
                }
                if let Some(key) = named(dep).filter(|_| at != place) {
                    moved.push((key.to_owned(), at, place));
                }
                place += 1;
            }
            for (key, at, _) in &moved {
                self.entries.remove(&(key.clone(), uid, *at));
            }
            for (key, _, place) in moved {
                self.entries.insert((key, uid, place));
            }
            issue.dangling.retain(|dep| !taken(dep));
        }
    }
}
