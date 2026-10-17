//! Where the imported dependencies that wait for a key are (roll-lqjj.18): indexed by the key each
//! names, so that a create visits the entries its key settles and no others.

use std::collections::{BTreeMap, BTreeSet};

use crate::id::EventId;
use crate::json::Value;
use crate::work::{Work, tally};

use super::Issue;

/// Every imported dependency still waiting, as the key it names, its issue and its number in that
/// issue's list ([`Issue::dangling`]). One that names its own issue's key waits for good, and is
/// listed too, so that a removal by key finds it. An entry's number is its place in the list when
/// the list began to wait, and it keeps it: an entry taken out while events apply leaves null in its
/// place, and [`Awaiting::settle`] makes each such list whole again, in its order, once the events
/// have applied, dropping the numbers of the entries taken with them, so that nothing listed is
/// listed again however many entries go from a list over however many batches. Ordered, so that what
/// one key settles is one range and nothing a hash would order reaches the state.
#[derive(Clone, Debug, Default)]
pub(crate) struct Awaiting {
    /// Key, issue and number.
    entries: BTreeSet<(String, EventId, usize)>,
    /// The number of each entry of an issue's list, in the list's order, which is theirs: for each
    /// issue whose list has entries.
    numbers: BTreeMap<EventId, Vec<usize>>,
    /// The issues an entry was taken from since the last [`Awaiting::settle`], as often as one was.
    unsettled: Vec<EventId>,
}

/// The key an imported dependency names: `None` for an entry taken out, which is null. Replay
/// reads a waiting entry's key here and nowhere else.
pub(crate) fn named(dep: &Value) -> Option<&str> {
    tally(Work::Waiting, 1);
    dep.get("depends_on_id").and_then(Value::as_str)
}

/// Whether the entry was taken out, and only holds its place.
fn taken(dep: &Value) -> bool {
    matches!(dep, Value::Null)
}

/// The place in `uid`'s list of its entry numbered `number`, by `numbers`.
fn place(numbers: &BTreeMap<EventId, Vec<usize>>, uid: EventId, number: usize) -> Option<usize> {
    numbers.get(&uid)?.binary_search(&number).ok()
}

impl Awaiting {
    /// Where every entry of `issues`' lists is, each list whole and numbered from its first entry:
    /// what a state read back starts with.
    pub(crate) fn of<'a>(issues: impl Iterator<Item = &'a Issue>) -> Awaiting {
        let mut awaiting = Awaiting::default();
        for issue in issues {
            awaiting.wait(issue);
        }
        awaiting
    }

    /// Lists the entries of `issue`, a list whole that has not waited before, numbered from its
    /// first entry.
    pub(crate) fn wait(&mut self, issue: &Issue) {
        if issue.dangling.is_empty() {
            return;
        }
        for (number, dep) in issue.dangling.iter().enumerate() {
            if let Some(key) = named(dep) {
                self.entries.insert((key.to_owned(), issue.uid, number));
            }
        }
        (self.numbers).insert(issue.uid, (0..issue.dangling.len()).collect());
    }

    /// Whether an entry waits for `key`: one of the keys the issues' lists name, entries taken out
    /// while events apply not among them, as [`Issue::dangling`] holds them.
    pub(crate) fn names(&self, key: &str) -> bool {
        let from = (key.to_owned(), EventId::from_u128(0), 0);
        (self.entries.range(from..).next()).is_some_and(|(named, _, _)| named == key)
    }

    /// The issue of every entry that waits for `key`, as often as it has one, still listed: those a
    /// create of the key makes an edge in.
    pub(crate) fn waiting(&self, key: &str) -> Vec<EventId> {
        let from = (key.to_owned(), EventId::from_u128(0), 0);
        let waiting: Vec<EventId> = (self.entries.range(from..))
            .take_while(|(named, _, _)| named == key)
            .map(|(_, uid, _)| *uid)
            .collect();
        tally(Work::Waiting, waiting.len());
        waiting
    }

    /// Every entry that waits for `key`, no longer listed: each issue's, by its number, in the order
    /// of its list.
    pub(crate) fn arrived(&mut self, key: &str) -> Vec<(EventId, usize)> {
        if self.entries.is_empty() {
            return Vec::new(); // most trackers: nothing an import named is missing
        }
        let range = (key.to_owned(), EventId::from_u128(0), 0)
            ..=(key.to_owned(), EventId::from_u128(u128::MAX), usize::MAX);
        let arrived: Vec<(EventId, usize)> = (self.entries.extract_if(range, |_| true))
            .map(|(_, uid, number)| (uid, number))
            .collect();
        tally(Work::Waiting, arrived.len());
        arrived
    }

    /// Takes out of `issue`'s list the entries that wait for `key` and that `chosen` picks.
    pub(crate) fn retract(
        &mut self,
        issue: &mut Issue,
        key: &str,
        chosen: impl Fn(&Value) -> bool,
    ) {
        let Awaiting {
            entries, numbers, ..
        } = self;
        let range = (key.to_owned(), issue.uid, 0)..=(key.to_owned(), issue.uid, usize::MAX);
        let retracted: Vec<usize> = (entries.extract_if(range, |(_, uid, number)| {
            tally(Work::Waiting, 1);
            (place(numbers, *uid, *number))
                .and_then(|at| issue.dangling.get(at))
                .is_some_and(&chosen)
        }))
        .map(|(_, _, number)| number)
        .collect();
        for number in retracted {
            self.take(issue, number);
        }
    }

    /// The entry of `issue`'s list numbered `number`, which is no longer listed, taken out of the
    /// list: null in its place until the list is settled. `None` when there is none.
    pub(crate) fn take(&mut self, issue: &mut Issue, number: usize) -> Option<Value> {
        let at = place(&self.numbers, issue.uid, number)?;
        let slot = issue.dangling.get_mut(at).filter(|dep| !taken(dep))?;
        self.unsettled.push(issue.uid);
        Some(std::mem::replace(slot, Value::Null))
    }

    /// Makes whole every list an entry was taken from, in its order, with the numbers of the entries
    /// left: what a state is finished with, so that no null is ever seen. The entries left keep
    /// their numbers, so nothing is listed again.
    pub(crate) fn settle(&mut self, issues: &mut super::Issues) {
        let mut unsettled = std::mem::take(&mut self.unsettled);
        unsettled.sort_unstable();
        unsettled.dedup();
        for uid in unsettled {
            let Some(issue) = issues.get_mut(&uid) else {
                continue;
            };
            tally(Work::Waiting, issue.dangling.len());
            if let Some(numbers) = self.numbers.get_mut(&uid) {
                let mut at = 0;
                numbers.retain(|_| {
                    let kept = issue.dangling.get(at).is_some_and(|dep| !taken(dep));
                    at += 1;
                    kept
                });
                if numbers.is_empty() {
                    self.numbers.remove(&uid);
                }
            }
            issue.dangling.retain(|dep| !taken(dep));
        }
    }

    /// Every entry listed, as its key, its issue and its place in the issue's list, `None` where
    /// that place does not hold an entry naming the key: in order. What two states that wait alike
    /// agree on, however each numbered its entries.
    #[cfg(test)]
    pub(crate) fn listed(
        &self,
        issues: &indexmap::IndexMap<EventId, Box<Issue>>,
    ) -> Vec<(String, EventId, Option<usize>)> {
        let mut listed: Vec<(String, EventId, Option<usize>)> = (self.entries.iter())
            .map(|(key, uid, number)| {
                let at = place(&self.numbers, *uid, *number).filter(|at| {
                    let dep = issues.get(uid).and_then(|issue| issue.dangling.get(*at));
                    dep.and_then(named) == Some(key.as_str())
                });
                (key.clone(), *uid, at)
            })
            .collect();
        listed.sort();
        listed
    }

    /// Every entry listed, as its key, its issue and its number.
    #[cfg(test)]
    pub(crate) fn numbered(&self) -> Vec<(String, EventId, usize)> {
        self.entries.iter().cloned().collect()
    }
}
