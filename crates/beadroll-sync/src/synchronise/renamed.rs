//! The renames a sync reports: every key its merge changed, against the renames the store's state
//! held when the merge first read it.

use std::collections::{HashMap, HashSet};

use beadroll_core::id::EventId;
use beadroll_core::json::Value;
use beadroll_core::state::{Rename, State};

/// Every key the merge changed, as renames, in replay order: each issue renamed now whose key is
/// other than `before` held (the renames the store's state had when the merge read it), from the
/// key it had then, or the key its create gave when it had none; and each issue renamed in
/// `before` that holds its own key again, from the key it had to the one it has. Every rename
/// when `before` was not read.
pub(super) fn renamed_by_merge(
    state: &State,
    before: Option<&HashMap<EventId, Rename>>,
) -> Vec<Value> {
    let Some(before) = before else {
        return (state.renames().iter())
            .map(|r| Value::from(r.to_value()))
            .collect();
    };
    let mut changed: Vec<Rename> = (state.renames().iter())
        .filter_map(|now| match before.get(&now.uid) {
            Some(had) if had.to == now.to => None,
            Some(had) => Some(Rename {
                from: had.to.clone(),
                ..now.clone()
            }),
            None => Some(now.clone()),
        })
        .collect();
    let still: HashSet<EventId> = state.renames().iter().map(|r| r.uid).collect();
    changed.extend(
        (before.values())
            .filter(|had| !still.contains(&had.uid))
            .filter_map(|had| {
                let key = state.issue(had.uid)?.key();
                (key != had.to).then(|| Rename {
                    from: had.to.clone(),
                    to: key.to_owned(),
                    ..had.clone()
                })
            }),
    );
    if changed.len() > 1 {
        let at: HashMap<EventId, usize> = (state.issues().enumerate())
            .map(|(n, issue)| (issue.uid(), n))
            .collect();
        changed.sort_by_key(|r| at.get(&r.uid).copied());
    }
    changed
        .into_iter()
        .map(|r| Value::from(r.to_value()))
        .collect()
}
