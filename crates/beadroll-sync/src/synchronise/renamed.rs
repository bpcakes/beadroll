//! The renames a sync reports: every key its merge changed, against the keys the store's state held
//! when the merge first read it.

use std::collections::HashMap;

use beadroll_core::id::EventId;
use beadroll_core::json::Value;
use beadroll_core::state::{Rename, State};

/// Every issue's key in the store's state as a reporting sync's first merge to write read it, before
/// it wrote. A merge may change a key with no rename before or after it (a foreign journal replaced
/// by the published one, an import holding its own key again), so every key is kept, not only the
/// renamed ones; a sync whose merges wrote nothing keeps none.
#[derive(Debug, Default)]
pub(crate) struct KeysBefore {
    keys: HashMap<EventId, String>,
}

impl KeysBefore {
    /// The keys `state` holds, moved out of it: a merge about to write, which goes on with the
    /// state it writes, has no more use for the one it began with.
    pub(crate) fn taken(state: State) -> KeysBefore {
        KeysBefore {
            keys: (state.into_keys().into_iter())
                .map(|(key, uid)| (uid, key))
                .collect(),
        }
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
    // the renames are in replay order, at most one an issue, as the issues are: walked alongside
    let mut renames = state.renames().iter().peekable();
    let mut changed = Vec::new();
    for issue in state.issues() {
        let (uid, now) = (issue.uid(), issue.key());
        let renamed = renames.next_if(|rename| rename.uid == uid);
        let rename = match before.keys.get(&uid) {
            Some(had) if had == now => continue,
            Some(had) => Rename {
                from: had.clone(),
                to: now.to_owned(),
                replica: issue.origin_machine().to_owned(),
                uid,
            },
            None => match renamed {
                Some(rename) => (*rename).clone(),
                None => continue,
            },
        };
        changed.push(Value::from(rename.to_value()));
    }
    changed
}
