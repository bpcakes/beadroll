//! The renames a sync reports: every key its merge changed, against the keys the store's state held
//! when the merge first read it.

use std::collections::HashMap;

use beadroll_core::id::EventId;
use beadroll_core::json::Value;
use beadroll_core::state::{Keys, Rename, State};

/// Every issue's key in the store's state as a reporting sync's first merge to write read it, before
/// it wrote. A merge may change a key with no rename before or after it (a foreign journal replaced
/// by the published one, an import holding its own key again), so every key is kept, not only the
/// renamed ones; a sync whose merges wrote nothing keeps none.
#[derive(Debug, Default)]
pub(crate) struct KeysBefore {
    keys: Keys,
    /// The renames the state held, in replay order.
    renames: Vec<Rename>,
    /// Whether a key may have changed with no rename on either side: a merge since rewrote a
    /// journal file the store held (another machine's put back as published, an attempt file),
    /// or a round gave way, after which another process may have done so.
    unsettled: bool,
}

impl KeysBefore {
    /// The keys `state` holds, moved out of it: a merge about to write, which goes on with the
    /// state it writes, has no more use for the one it began with.
    /// Kept as the state held them, the index each key's issue is found by made only when the
    /// report is, once the sync has let go of the store.
    pub(crate) fn taken(state: State) -> KeysBefore {
        let (keys, renames) = state.into_keys();
        KeysBefore {
            keys,
            renames,
            unsettled: false,
        }
    }

    /// A key may have changed with no rename either side: the report compares every one.
    pub(crate) fn unsettle(&mut self) {
        self.unsettled = true;
    }
}

/// Every key the sync's merges changed, as renames, in replay order: each issue held `before` whose
/// key is another now, from the key it had to the one it has, its machine the create's that made it
/// now; and each issue new to this sync that took another key than its create gave, from that one.
/// None when no merge wrote (`before` was not kept): the state is the store's own, its keys as they
/// were.
pub(super) fn renamed_by_merge(state: &State, before: Option<&KeysBefore>) -> Vec<Value> {
    let Some(before) = before else {
        return Vec::new();
    };
    // the renames as they were, more after them perhaps, and no journal file rewritten nor a round
    // given way since: an issue renamed before holds the key it did, and every one renamed after,
    // absent then or holding its create's key, is renamed from the key it had; the rest hold their
    // creates' keys as they did. The renames after are the report
    if !before.unsettled && state.renames().starts_with(&before.renames) {
        return (state.renames()[before.renames.len()..].iter())
            .map(|rename| Value::from(rename.to_value()))
            .collect();
    }
    // indexed are only the issues whose key moved: an issue the merge left alone holds its key in
    // `before` still, and one new to the sync holds none there
    let moved: HashMap<EventId, &str> = (before.keys.iter())
        .filter(|(key, uid)| {
            state
                .issue(**uid)
                .is_some_and(|issue| issue.key() != key.as_str())
        })
        .map(|(key, uid)| (*uid, key.as_str()))
        .collect();
    // no key the store held moved: only issues new to the sync can have taken another key than
    // their create gave, the renames whose issue did not hold that key before, in replay order
    if moved.is_empty() {
        return (state.renames().iter())
            .filter(|rename| before.keys.get(&rename.to) != Some(&rename.uid))
            .map(|rename| Value::from(rename.to_value()))
            .collect();
    }
    // the renames are in replay order, at most one an issue, as the issues are: walked alongside
    let mut renames = state.renames().iter().peekable();
    let mut changed = Vec::new();
    for issue in state.issues() {
        let (uid, now) = (issue.uid(), issue.key());
        let renamed = renames.next_if(|rename| rename.uid == uid);
        let rename = match moved.get(&uid) {
            Some(had) => Rename {
                from: (*had).to_owned(),
                to: now.to_owned(),
                replica: issue.origin_machine().to_owned(),
                uid,
            },
            None if before.keys.get(now) == Some(&uid) => continue,
            // new to the sync
            None => match renamed {
                Some(rename) => (*rename).clone(),
                None => continue,
            },
        };
        changed.push(Value::from(rename.to_value()));
    }
    changed
}
