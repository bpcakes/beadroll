//! A state's issues, in the order replay created them, and which of them changed since the state was
//! read back from a projection's bytes (roll-lqjj.44): a write-back writes those again and may copy
//! the bytes of the others as they were read. Every way to change an issue goes through [`Issues`],
//! which marks it, but for the two that change nothing a write-back writes: reading its texts in
//! ([`Issues::read_in`]) and deriving its labels ([`Issues::relabel`]). An issue is never removed
//! or moved: [`Issues`] has no way to, and lends the map only to read, so each issue read back stays
//! at the position its bytes were read from.

use std::ops::Deref;

use indexmap::IndexMap;

use super::issue::Issue;
use super::texts::Texts;
use crate::id::EventId;
#[cfg(test)]
use crate::journal::JournalOrder;
#[cfg(test)]
use crate::projection::tests::HistoryKind;

/// The issues, by id, in the order replay created them, with which of those read back changed.
#[derive(Clone, Debug, Default)]
pub(crate) struct Issues {
    /// Boxed, so that what the map keeps spare as it grows is a pointer a place, not an issue.
    map: IndexMap<EventId, Box<Issue>>,
    /// How many issues, from the first, were read back from a projection's bytes: none in a state
    /// replay made.
    read: usize,
    /// Which of those changed since, a bit each.
    marks: Vec<u64>,
}

impl Deref for Issues {
    type Target = IndexMap<EventId, Box<Issue>>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl Issues {
    /// Marks the issue at `at` as changed.
    fn mark(&mut self, at: usize) {
        if let Some(word) = self.marks.get_mut(at / 64) {
            *word |= 1 << (at % 64);
        }
    }

    /// Issue `uid`, to change: marked.
    pub(crate) fn get_mut(&mut self, uid: &EventId) -> Option<&mut Issue> {
        let (at, _, issue) = self.map.get_full_mut(uid)?;
        if let Some(word) = self.marks.get_mut(at / 64) {
            *word |= 1 << (at % 64);
        }
        Some(issue)
    }

    /// Every issue, to change: all marked.
    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut Issue> {
        self.marks.fill(u64::MAX);
        self.map.values_mut().map(|issue| &mut **issue)
    }

    /// Room for exactly `n` more issues, so that a state read back is not grown as it is read, nor
    /// given room it does not use.
    pub(crate) fn reserve(&mut self, n: usize) {
        self.map.reserve_exact(n);
    }

    /// Adds `issue` as `uid` after every other; one already there is replaced where it is, marked.
    pub(crate) fn insert(&mut self, uid: EventId, issue: Box<Issue>) {
        let (at, _) = self.map.insert_full(uid, issue);
        self.mark(at);
    }

    /// Issue `uid`, to read its texts in, unmarked: reading them in changes nothing a write-back
    /// writes of it, which the differential tests check of every issue they read texts into
    /// (`projection/tests/marked.rs`). For [`super::State::read_texts`] alone.
    pub(crate) fn read_in(&mut self, uid: &EventId) -> Option<&mut Issue> {
        self.map.get_mut(uid).map(|issue| &mut **issue)
    }

    /// Derives issue `uid`'s labels again, unmarked: no write-back writes them.
    pub(crate) fn relabel(&mut self, uid: &EventId) {
        if let Some(issue) = self.map.get_mut(uid) {
            issue.relabel();
        }
    }

    /// Derives every issue's labels again, unmarked.
    pub(crate) fn relabel_all(&mut self) {
        for issue in self.map.values_mut() {
            issue.relabel();
        }
    }

    /// Holds every issue as read back from a projection's bytes, none changed since: what a state
    /// read back is once it is read.
    pub(crate) fn hold_as_read(&mut self) {
        self.read = self.map.len();
        self.marks = vec![0; self.read.div_ceil(64)];
    }

    /// Whether the issue at `at` is to be written again rather than copied as it was read back: it
    /// was not read back, or it changed since, or its texts are to be placed anew, which gives it
    /// a place no bytes read back hold.
    pub(crate) fn changed(&self, at: usize) -> bool {
        let marked = (self.marks.get(at / 64)).is_none_or(|word| word & (1 << (at % 64)) != 0);
        at >= self.read
            || marked
            || (self.map.get_index(at)).is_some_and(|(_, issue)| matches!(issue.texts, Texts::Here))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Journal;
    use crate::projection::tests::history;
    use crate::replay::replay;

    /// The issues of a clean differential history, held as read back.
    fn read_back() -> Issues {
        let mut journal = Journal::new();
        for record in history(3, 30, HistoryKind::Valid) {
            journal.add(
                record.value,
                &record.replica,
                &record.origin,
                if record.sequenced {
                    JournalOrder::Sequenced
                } else {
                    JournalOrder::Attempt
                },
            );
        }
        let mut issues = replay(journal).unwrap().issues;
        for issue in issues.values_mut() {
            issue.texts = Texts::Read(crate::state::TextPlace {
                at: 0,
                len: 0,
                sum: [0; 16],
            });
        }
        issues.hold_as_read();
        issues
    }

    fn marked(issues: &Issues) -> Vec<usize> {
        (0..issues.len()).filter(|at| issues.changed(*at)).collect()
    }

    #[test]
    fn what_changes_an_issue_marks_it_and_nothing_else_does() {
        let mut issues = read_back();
        let count = issues.len();
        assert!(count > 20, "{count} issues");
        assert_eq!(marked(&issues), [] as [usize; 0]);
        let uid = |issues: &Issues, at: usize| *issues.get_index(at).unwrap().0;
        // changed through get_mut: marked, every other as it was
        let third = uid(&issues, 3);
        issues.get_mut(&third).unwrap();
        assert_eq!(marked(&issues), [3]);
        // texts read in, labels derived: unmarked
        let fifth = uid(&issues, 5);
        issues.read_in(&fifth).unwrap();
        issues.relabel(&fifth);
        issues.relabel_all();
        assert_eq!(marked(&issues), [3]);
        // texts to be placed anew: changed, though no mark says so
        issues.read_in(&fifth).unwrap().texts = Texts::Here;
        assert_eq!(marked(&issues), [3, 5]);
        // the same id inserted again: replaced where it is, and marked
        let seventh = uid(&issues, 7);
        let again = issues.get(&seventh).unwrap().clone();
        issues.insert(seventh, again);
        assert_eq!(issues.len(), count);
        assert_eq!(uid(&issues, 7), seventh);
        assert_eq!(marked(&issues), [3, 5, 7]);
        // a new issue: after every other, changed
        let mut new = issues.get(&seventh).unwrap().clone();
        new.uid = EventId::from_u128(1);
        issues.insert(new.uid, new);
        assert_eq!(marked(&issues), [3, 5, 7, count]);
        // every issue to change: all marked
        issues.values_mut().for_each(drop);
        assert_eq!(marked(&issues), (0..=count).collect::<Vec<_>>());
        // held as read again: none
        issues.hold_as_read();
        let none: Vec<usize> = (0..=count).filter(|at| issues.changed(*at)).collect();
        assert_eq!(none, [5]); // its texts are still to be placed anew
    }

    #[test]
    fn a_state_replay_made_has_every_issue_changed() {
        let mut journal = Journal::new();
        for record in history(3, 10, HistoryKind::Valid) {
            journal.add(
                record.value,
                &record.replica,
                &record.origin,
                if record.sequenced {
                    JournalOrder::Sequenced
                } else {
                    JournalOrder::Attempt
                },
            );
        }
        let issues = replay(journal).unwrap().issues;
        assert!(!issues.is_empty());
        assert!((0..issues.len()).all(|at| issues.changed(at)));
    }
}
