//! The imported dependencies that wait for keys (roll-lqjj.18): a create visits the entries its key
//! settles and a removal by key the entries for that key, and nothing else waiting.

use super::super::Work;
use super::{History, grows_linearly, key, one_at_a_time};
use crate::json::{Object, Value};
use crate::replay::replay;

/// Whether generated dependencies name keys later brought by the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AwaitedKeys {
    /// Missing.
    Missing,
    /// Brought.
    Brought,
}

/// The history of `issues` creates whose imports each wait for a key no issue has: the first
/// half's for the keys the second half brings when `brought`, and for keys that never come
/// otherwise.
fn waiting(issues: usize, brought: AwaitedKeys) -> History {
    let brought = matches!(brought, AwaitedKeys::Brought);
    let mut history = History::new();
    let half = issues / 2;
    for n in 0..issues {
        let named = if brought && n < half {
            key(n + half)
        } else {
            format!("ex-gone{n}")
        };
        history.create(&key(n), &[(named, "blocks")], &[]);
    }
    history
}

#[test]
fn creates_whose_imports_wait_for_keys_visit_their_own_entries_alone() {
    grows_linearly(
        "replaying creates each waiting for a key that never comes",
        &[Work::Waiting],
        |issues| waiting(issues, AwaitedKeys::Missing).journal(),
        |journal| drop(replay(journal).unwrap()),
    );
    grows_linearly(
        "replaying creates, half waiting for the keys the other half brings",
        &[Work::Waiting],
        |issues| waiting(issues, AwaitedKeys::Brought).journal(),
        |journal| drop(replay(journal).unwrap()),
    );
}

#[test]
fn a_projection_taking_awaited_keys_one_create_a_batch_visits_their_entries_alone() {
    let build = |issues: usize| {
        let mut history = History::new();
        for n in 0..issues {
            history.create(&key(n), &[(format!("ex-later{n}"), "blocks")], &[]);
        }
        let held = history.events.len();
        let projection = history.projection();
        for n in 0..issues {
            history.create(&format!("ex-later{n}"), &[], &[]);
        }
        (projection, history.after(held))
    };
    grows_linearly(
        "bringing the key every issue waits for, one create at a time",
        &[Work::Waiting],
        build,
        |(projection, creates)| drop(one_at_a_time(projection, creates)),
    );
}

#[test]
fn removals_by_key_visit_the_entries_for_that_key_alone() {
    let build = |keys: usize| {
        let mut history = History::new();
        let named: Vec<(String, &str)> = (0..keys).map(|n| (key(n), "blocks")).collect();
        let issue = history.create("ex-waits", &named, &[]);
        for (named, kind) in named {
            let mut data = Object::new();
            data.insert("key".into(), Value::from(named));
            data.insert("type".into(), Value::from(kind));
            history.add("dep_remove", Some(issue), data);
        }
        history.journal()
    };
    grows_linearly(
        "removing every dependency one issue waits for by its key, one at a time",
        &[Work::Waiting],
        build,
        |journal| {
            let state = replay(journal).unwrap();
            assert!(state.issues().all(|issue| issue.dangling().is_empty()));
        },
    );
}
