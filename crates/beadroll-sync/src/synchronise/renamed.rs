//! The renames a sync reports: every key its merge changed, against the keys the store's state held
//! when the merge first read it.

use std::collections::HashMap;

use beadroll_core::id::EventId;
use beadroll_core::json::Value;
use beadroll_core::state::{Rename, State};

/// What a reporting sync's first merge read of the store's state before merging: every issue's key,
/// and the machine of each rename it held, by issue. A merge may change a key with no rename
/// before or after it (a foreign journal replaced by the published one, an import holding its own
/// key again), so every key is kept, not only the renamed ones.
#[derive(Debug, Default)]
pub(crate) struct KeysBefore {
    keys: HashMap<EventId, String>,
    renamed_by: HashMap<EventId, String>,
}

impl KeysBefore {
    /// The keys `state` holds, and the machines of its renames.
    pub(crate) fn of(state: &State) -> KeysBefore {
        KeysBefore {
            keys: (state.keys().iter())
                .map(|(key, uid)| (*uid, key.clone()))
                .collect(),
            renamed_by: (state.renames().iter())
                .map(|rename| (rename.uid, rename.replica.clone()))
                .collect(),
        }
    }
}

/// Every key the merge changed, as renames, in replay order: each issue held `before` whose key is
/// another now, from the key it had to the one it has, and each issue new to this sync that took
/// another key than its create gave, from that one. Every rename when `before` was not read.
pub(super) fn renamed_by_merge(state: &State, before: Option<&KeysBefore>) -> Vec<Value> {
    let Some(before) = before else {
        return (state.renames().iter())
            .map(|r| Value::from(r.to_value()))
            .collect();
    };
    let renames: HashMap<EventId, &Rename> = (state.renames().iter())
        .map(|rename| (rename.uid, rename))
        .collect();
    let mut changed = Vec::new();
    for issue in state.issues() {
        let (uid, now) = (issue.uid(), issue.key());
        let renamed = renames.get(&uid);
        let rename = match before.keys.get(&uid) {
            Some(had) if had == now => continue,
            Some(had) => Rename {
                from: had.clone(),
                to: now.to_owned(),
                replica: (renamed.map(|r| r.replica.as_str()))
                    .or_else(|| before.renamed_by.get(&uid).map(String::as_str))
                    .unwrap_or_else(|| issue.origin_machine())
                    .to_owned(),
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
