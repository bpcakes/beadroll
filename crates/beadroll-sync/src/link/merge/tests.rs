//! A completed merge remains its own snapshot while the link fails and retries later rounds.

use std::sync::Arc;

use beadroll_core::json::Object;
use beadroll_store::{Endpoint, Settings, Workspace};
use beadroll_test_support::{Machine, Sandbox, built_bead};

use super::{Link, attempt_shaped, segment_shaped, stem};
use crate::Error;
use crate::locked;
use crate::synchronise::SyncReport;
use std::path::Path;

#[test]
fn completed_merges_keep_their_state_and_counts_across_failed_rounds() {
    let sandbox = Sandbox::new(built_bead());
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "first"]);
    let settings = Settings::from_process(
        a.env(None, "codex"),
        Some(a.checkout().to_path_buf()),
        std::env::var_os("HOME").unwrap_or_default(),
        std::env::temp_dir(),
        &|_| None,
    )
    .unwrap();
    let store = a.store();
    let ws = Workspace::new(
        Arc::new(settings),
        store.file_name().unwrap().to_str().unwrap(),
        None,
        Some(sandbox.origin().as_os_str().to_owned()),
        Object::new(),
    )
    .unwrap();
    let mut link = Link::new(&ws, &Endpoint::at(sandbox.origin())).unwrap();
    let first = locked(&ws, || -> Result<_, Error> {
        let tip = link.fetch()?;
        link.merge_from(tip.as_deref(), SyncReport::Collect)
    })
    .unwrap();
    assert_eq!(first.state.state().issues().count(), 1);
    assert_eq!(first.pulled, 0);
    let original_counts = first.facts.remote_counts.clone();

    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    b.bead(["create", "second"]);
    locked(&ws, || -> Result<(), Error> {
        assert!(
            link.merge_from(Some("missing-tip"), SyncReport::Collect)
                .is_err()
        );
        let tip = link.fetch()?;
        let second = link.merge_from(tip.as_deref(), SyncReport::Collect)?;
        assert_eq!(
            (second.pulled, second.state.state().issues().count()),
            (1, 2)
        );
        assert_eq!(link.unpublished_after(&second)?, 0);
        assert_ne!(second.facts.remote_counts, original_counts);
        assert_eq!(first.state.state().issues().count(), 1);
        assert_eq!(first.facts.remote_counts, original_counts);
        // The compatibility API still exposes the latest merge's report on Link.
        assert_eq!(link.merge(tip.as_deref())?, 0);
        assert_eq!(link.remote_counts, second.facts.remote_counts);
        assert_eq!(link.unpublished()?, 0);
        Ok(())
    })
    .unwrap();
}

#[test]
fn published_paths_are_told_apart_by_their_shape() {
    assert!(attempt_shaped("events/ab12cd34/claims/0190-ab.json"));
    assert!(!attempt_shaped("events/ab12cd34/claims/0190-AB.json"));
    assert!(!attempt_shaped("events/ab12cd34/claims/.json"));
    assert!(!attempt_shaped("events//claims/x.json"));
    assert!(segment_shaped("events/ab12cd34/000001.jsonl"));
    assert!(!segment_shaped("events/ab12cd34/00a001.jsonl"));
    assert!(!segment_shaped("events/ab12cd34/claims/000001.jsonl"));
    assert_eq!(stem(Path::new("a/0190-ab.json")), "0190-ab");
    assert_eq!(stem(Path::new("a/.json")), ".json");
}
