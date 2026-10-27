//! The numbers renamed copies found claimed (roll-lqjj.46): a copy of a numbered key takes the
//! first number at or above its own whose key no issue has and no import waits for
//! (`key::free_key`), and two machines that both mint the children `ex-1.1` to `ex-1.N` give N
//! copies that each count past nearly every number the others did, N² keys asked of in all. The
//! numbers asked of and found claimed are kept here as runs, so that the next copy skips a run in
//! one step and asks of the number after it, and N copies ask of about 3N keys.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use crate::key;
use crate::work::{Work, tally};

/// A key's number as text in its shortest form: ASCII digits, the first of them not 0 unless it is
/// the only one. Ordered as the numbers are, a longer one larger, of any length, so that nothing
/// bead counts depends on the range of an integer (AD01).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Number(String);

impl Number {
    /// The number after it, as long or one digit longer.
    fn next(&self) -> Number {
        Number(key::next_number(&self.0))
    }
}

impl Ord for Number {
    fn cmp(&self, other: &Number) -> Ordering {
        (self.0.len().cmp(&other.0.len())).then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for Number {
    fn partial_cmp(&self, other: &Number) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// For each stem, the runs of consecutive numbers whose keys renamed copies found claimed, each
/// from its first number to the number after its last, no two touching. Every number in a run is
/// claimed: a number joins a run only once [`super::State::claims_key`] answered so of its key, and
/// leaves it when an import that waited for the key is taken back ([`Runs::forget`]), the one way
/// a key stops being claimed, since an issue keeps its key for good and an import's key that
/// arrives becomes the arriving issue's. Derived and never written: a state starts with none, and
/// so does one read back, whose first copy asks of each number as copies did before.
#[derive(Clone, Debug, Default)]
pub(crate) struct Runs {
    stems: HashMap<String, BTreeMap<Number, Number>>,
}

impl Runs {
    /// The key under `stem` of the first number at or above `digits`' (ASCII digits, leading zeros
    /// not counted) that `claimed` says is free: what [`key::free_key`] counts to one number at a
    /// time, here skipping each run of numbers known to be claimed, and keeping each number asked
    /// of and found claimed in a run.
    pub(crate) fn free(
        &mut self,
        stem: &str,
        digits: &str,
        claimed: impl Fn(&str) -> bool,
    ) -> String {
        // past the run it starts in, if any; then the number after each run met is in none, runs
        // never touching
        let mut number = self.skip(stem, Number(key::counted_from(digits).to_owned()));
        loop {
            let candidate = format!("{stem}.{}", number.0);
            if !claimed(&candidate) {
                return candidate;
            }
            number = self.learn(stem, number);
        }
    }

    /// The number after the run under `stem` that holds `number`, or `number` when none does.
    fn skip(&self, stem: &str, number: Number) -> Number {
        tally(Work::Key, 1);
        let run = (self.stems.get(stem)).and_then(|runs| runs.range(..=&number).next_back());
        match run {
            Some((_, end)) if number < *end => {
                #[cfg(test)]
                used::count(|used| used.skipped += 1);
                end.clone()
            }
            _ => number,
        }
    }

    /// Keeps `number`, claimed under `stem`, in a run, joined with the run that ends at it and the
    /// one that starts after it; returns the number after the run it is in. A number already in a
    /// run, which the walk never learns, leaves its run as it is.
    fn learn(&mut self, stem: &str, number: Number) -> Number {
        tally(Work::Key, 1);
        if !self.stems.contains_key(stem) {
            self.stems.insert(stem.to_owned(), BTreeMap::new());
        }
        let after = number.next();
        let Some(runs) = self.stems.get_mut(stem) else {
            return after;
        };
        let start = match runs.range(..=&number).next_back() {
            Some((_, end)) if number < *end => return end.clone(),
            Some((start, end)) if *end == number => start.clone(),
            _ => number,
        };
        let end = runs.remove(&after).unwrap_or(after);
        runs.insert(start, end.clone());
        end
    }

    /// Takes `key`'s number out of the run that holds it, if any: `key` may be free now, an import
    /// that waited for it taken back. A key whose number has a leading zero is never a copy's, and
    /// is in no run.
    pub(crate) fn forget(&mut self, key: &str) {
        let Some((stem, digits)) = key::split_number(key) else {
            return;
        };
        let Some(runs) = self.stems.get_mut(stem) else {
            return;
        };
        if key::counted_from(digits) != digits {
            return;
        }
        let number = Number(digits.to_owned());
        let run = runs.range(..=&number).next_back();
        let Some((start, end)) = run.filter(|(_, end)| number < **end) else {
            return;
        };
        tally(Work::Key, 1);
        #[cfg(test)]
        used::count(|used| used.split += 1);
        let (start, end) = (start.clone(), end.clone());
        let after = number.next();
        if start == number {
            runs.remove(&start);
        } else {
            runs.insert(start, number);
        }
        if after < end {
            runs.insert(after, end);
        }
        if runs.is_empty() {
            self.stems.remove(stem);
        }
    }

    /// Every run, as its stem, its first number and the number after its last, in order.
    #[cfg(test)]
    pub(crate) fn listed(&self) -> Vec<(String, String, String)> {
        let mut listed: Vec<(String, String, String)> = (self.stems.iter())
            .flat_map(|(stem, runs)| {
                (runs.iter()).map(|(start, end)| (stem.clone(), start.0.clone(), end.0.clone()))
            })
            .collect();
        listed.sort_by(|a, b| {
            let number = |text: &str| Number(text.to_owned());
            (a.0.cmp(&b.0)).then_with(|| number(&a.1).cmp(&number(&b.1)))
        });
        listed
    }
}

/// How many numbers checking a copy's key counts one at a time at most ([`agrees`]), so that the
/// check costs a bounded amount for each copy however many numbers the copies count past.
#[cfg(test)]
pub(crate) const CHECKED: usize = 512;

/// Fails unless `free`, the key [`super::State::free_key`] picked for a copy of `key`, is the key
/// [`key::free_key`] picks counting one number at a time, asked of each key directly so that
/// nothing it reads is counted or kept: the same key, or, when that rule counts past [`CHECKED`]
/// numbers, a key under the same stem whose number is past every one it counted.
#[cfg(test)]
pub(super) fn agrees(state: &super::State, key: &str, uid: crate::id::EventId, free: Option<&str>) {
    let counted = std::cell::Cell::new(0_usize);
    let claimed = |candidate: &str| {
        counted.set(counted.get() + 1);
        counted.get() <= CHECKED
            && (state.by_key.contains_key(candidate) || state.awaiting.names(candidate))
    };
    let rule = key::free_key(key, &uid.to_string(), &claimed);
    if counted.get() <= CHECKED {
        assert_eq!(free, rule.as_deref(), "the key a copy of {key:?} takes");
        used::count(|used| used.compared += 1);
        return;
    }
    let number = |key: Option<&str>| {
        let (stem, digits) = key.and_then(key::split_number).unwrap_or_default();
        (stem.to_owned(), Number(digits.to_owned()))
    };
    let (taken, bound) = (number(free), number(rule.as_deref()));
    assert!(
        taken.0 == bound.0 && taken.1 >= bound.1,
        "a copy of {key:?} took {free:?}, which is not past {rule:?}"
    );
}

/// What runs did in this crate's tests: so that a test can show its states skipped and split runs,
/// and had their keys checked against the rule.
#[cfg(test)]
pub(crate) mod used {
    use std::cell::Cell;

    /// Counts of what runs did on this thread since they were last taken.
    #[derive(Clone, Copy, Debug, Default)]
    pub(crate) struct Used {
        /// Copies that skipped a run.
        pub(crate) skipped: usize,
        /// Runs split, an import's key taken back from inside one.
        pub(crate) split: usize,
        /// Copies' keys checked against the rule counting one number at a time, whole.
        pub(crate) compared: usize,
    }

    thread_local! {
        static USED: Cell<Used> = const {
            Cell::new(Used { skipped: 0, split: 0, compared: 0 })
        };
    }

    /// Counts what `change` adds.
    pub(super) fn count(change: impl FnOnce(&mut Used)) {
        USED.with(|cell| {
            let mut used = cell.get();
            change(&mut used);
            cell.set(used);
        });
    }

    /// What runs did on this thread since this was last asked.
    pub(crate) fn taken() -> Used {
        USED.with(Cell::take)
    }
}

#[cfg(test)]
mod tests;
