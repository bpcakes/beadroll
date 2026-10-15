//! The work done by the algorithms that roll-lqjj.15 to .20 made grow with their input rather than
//! with its square, counted where it is done, so that a change making one of them quadratic again
//! fails a test rather than passing every test that compares answers (roll-lqjj.34, X15). Each
//! [`tally`] is a call that does nothing outside this crate's own tests: only `cfg(test)` compiles
//! the counter, so a build of bead carries none of it. The guards in `work/tests.rs` count one
//! algorithm's work on an input and on one eight times its size and allow twice the growth of a
//! linear algorithm, sixteen times, where one that grows with the square of its input does
//! sixty-four.
//!
//! A count sees only the work that passes the place it is kept, which is chosen as the one every
//! way of doing that work passes: the edges an issue holds when they are asked for, an imported
//! dependency's key when it is read, the key of an import's record or dependency, the key a renamed
//! copy asks the state of. A change that does the work some other way, a lookup turned back into a
//! scan of a vector, is seen by time alone: `xtask`'s growth tests time those in release, every
//! night.

/// A kind of work counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Work {
    /// An edge read: each dependency of an issue whose edges are asked for, and each child the
    /// parent-child index lists for a parent asked about.
    Edge,
    /// An issue whose labels are derived again from its tags: one for it and one for each label
    /// it has held.
    Relabel,
    /// An issue whose edges the parent-child index is built from.
    Index,
    /// An imported dependency that waits for a key, visited: its key read, its entry handed to the
    /// create its key settles or examined for a removal, or kept when its list is made whole.
    Waiting,
    /// The key of an import's record, or the key one of its dependencies names, read; a key asked
    /// of whether an issue or an import claims it, and each key collected as claimed.
    Key,
}

/// Counts `n` units of `work`, in this crate's tests; anywhere else it does nothing.
#[inline(always)]
pub(crate) fn tally(work: Work, n: usize) {
    #[cfg(test)]
    counter::add(work, n);
    #[cfg(not(test))]
    let _ = (work, n);
}

#[cfg(test)]
pub(crate) use counter::counted;

/// The counter: one count of each kind for each thread, so that tests running beside each other
/// in one process count their own work alone.
#[cfg(test)]
mod counter {
    use std::cell::Cell;

    use super::Work;

    /// How many kinds of work there are.
    const KINDS: usize = 5;

    thread_local! {
        static COUNTS: [Cell<usize>; KINDS] = const { [const { Cell::new(0) }; KINDS] };
    }

    /// The place of `work`'s count.
    fn slot(work: Work) -> usize {
        match work {
            Work::Edge => 0,
            Work::Relabel => 1,
            Work::Index => 2,
            Work::Waiting => 3,
            Work::Key => 4,
        }
    }

    /// Counts `n` more of `work`.
    pub(super) fn add(work: Work, n: usize) {
        COUNTS.with(|counts| {
            let count = &counts[slot(work)];
            count.set(count.get().saturating_add(n));
        });
    }

    /// The work of each kind done on this thread so far.
    fn now() -> [usize; KINDS] {
        COUNTS.with(|counts| counts.each_ref().map(Cell::get))
    }

    /// The work of each kind one call did.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Counted([usize; KINDS]);

    impl Counted {
        /// How much of `work` it did.
        pub(crate) fn of(self, work: Work) -> usize {
            self.0[slot(work)]
        }
    }

    /// What `call` returns, and the work it did on this thread.
    pub(crate) fn counted<T>(call: impl FnOnce() -> T) -> (T, Counted) {
        let before = now();
        let answer = call();
        let after = now();
        (
            answer,
            Counted(std::array::from_fn(|at| after[at] - before[at])),
        )
    }
}

#[cfg(test)]
mod tests;
