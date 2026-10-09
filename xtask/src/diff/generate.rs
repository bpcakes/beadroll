//! Generated histories for `diff replay`, valid and hostile (`docs/RUST-PORT.md` §4.2 item 3).
//!
//! proptest draws a tape of random words for each case; a small model of machines writing,
//! syncing and claiming turns the tape into journals, the way `Tx.emit` writes them, then
//! [`hostile`] mutates envelopes, payloads, duplicates, chains, ownership and bytes. Word 0 is
//! always the plainest choice, so a shrunk tape is a plain history.

mod hostile;
mod imports;
mod model;
mod scenarios;

use proptest::collection::vec;
use proptest::prelude::any;
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

use super::cases::Case;

/// Random words a case is made from: the model reads them in order, and 0 once they run out.
pub(crate) struct Tape {
    words: Vec<u64>,
    at: usize,
}

impl Tape {
    pub(crate) fn new(words: Vec<u64>) -> Tape {
        Tape { words, at: 0 }
    }

    pub(crate) fn word(&mut self) -> u64 {
        let word = self.words.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        word
    }

    /// A number below `n` (0 when `n` is 0).
    pub(crate) fn below(&mut self, n: usize) -> usize {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        if n == 0 {
            0
        } else {
            usize::try_from(self.word() % n).unwrap_or(0)
        }
    }

    /// True `percent` times in a hundred; never once the tape has run out.
    pub(crate) fn chance(&mut self, percent: u64) -> bool {
        self.word() % 100 >= 100 - percent.min(100)
    }

    /// One of `items`.
    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        let at = self.below(items.len());
        items.get(at)
    }
}

/// `count` cases from `seed`: the fixed scenarios first, then histories from the model, made as
/// they are taken.
pub(crate) fn cases(count: usize, seed: u64) -> impl Iterator<Item = Case> {
    let mut bytes = [0u8; 32];
    for (slot, byte) in bytes.iter_mut().zip(seed.to_le_bytes()) {
        *slot = byte;
    }
    let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &bytes);
    let mut runner = TestRunner::new_with_rng(Config::default(), rng);
    let tapes = vec(any::<u64>(), 500..8000);
    let mut index = 0;
    let histories = std::iter::from_fn(move || {
        let tree = tapes.new_tree(&mut runner).ok()?;
        let mut tape = Tape::new(tree.current());
        let case = model::history(&mut tape, &format!("seed {seed} case {index}"));
        index += 1;
        Some(case)
    });
    scenarios::all().into_iter().chain(histories).take(count)
}
