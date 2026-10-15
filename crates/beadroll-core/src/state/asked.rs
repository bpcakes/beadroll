//! What [`State::claims_key`] answers in this crate's tests: whether [`State::claimed_keys`] holds
//! the key, checked at every ask, wherever replay or a projection is when it asks (X03). Each ask
//! is kept, so that a test can show which its states made.

use std::cell::RefCell;

use crate::json::Value;

use super::State;

/// One ask of [`State::claims_key`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    /// The key asked of.
    pub(crate) key: String,
    /// Whether it is claimed.
    pub(crate) claimed: bool,
    /// Whether it is claimed only because an imported dependency waits for it: no issue has it.
    pub(crate) awaited: bool,
    /// Whether it was asked while an entry taken out of a list held its place, before the list was
    /// made whole again ([`super::Awaiting::settle`]): in the batch the entry was taken out in.
    pub(crate) mid_batch: bool,
}

thread_local! {
    static ASKED: RefCell<Vec<Ask>> = const { RefCell::new(Vec::new()) };
}

/// Fails unless `claimed`, what [`State::claims_key`] answered of `key`, is whether
/// [`State::claimed_keys`] holds it; keeps the ask.
pub(super) fn agrees(state: &State, key: &str, claimed: bool) {
    assert_eq!(
        claimed,
        state.collect_claimed_keys().contains(key),
        "whether {key:?} is claimed, asked of the key and of every key collected"
    );
    let mid_batch = (state.issues.values())
        .any(|issue| (issue.dangling.iter()).any(|dep| matches!(dep, Value::Null)));
    let ask = Ask {
        key: key.to_owned(),
        claimed,
        awaited: claimed && !state.by_key.contains_key(key),
        mid_batch,
    };
    ASKED.with(|asked| asked.borrow_mut().push(ask));
}

/// The asks this thread made since it last took them, in their order.
pub(crate) fn taken() -> Vec<Ask> {
    ASKED.with(|asked| std::mem::take(&mut *asked.borrow_mut()))
}
