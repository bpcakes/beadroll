//! A rejected publication contributes incoming progress but not its round's confirmations.

use std::path::{Path, PathBuf};

use beadroll_core::json::Value;
use beadroll_store::{Endpoint, claim_files, read_attempt};
use beadroll_sync::synchronise;
use beadroll_test_support::{Machine, git, json, json_line};

use super::damage_tip;
use crate::support::{Gate, alongside, sandbox, text, workspace};

/// A pending twin of an already published attempt models a lost confirmation before recovery.
fn pending_twin(store: &Path, accepted: &Path) -> PathBuf {
    let pending = store.join("pending");
    std::fs::create_dir_all(&pending).unwrap();
    let twin = pending.join(accepted.file_name().unwrap());
    std::fs::copy(accepted, &twin).unwrap();
    twin
}

#[test]
fn a_retry_reports_only_its_final_confirmations_and_all_incoming_events() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    let b = Machine::new(&sandbox, "b");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "owned here"]).unwrap()["id"]);
    a.bead(["claim", &key]);
    a.bead(["release", &key]);
    b.bead(["sync"]);
    // Each round receives a new sequential event; a's edit requires an actual push.
    a.bead(["create", "local edit", "--offline"]);
    b.bead(["create", "before the first merge"]);

    let publishing = Gate::new(&sandbox, "publishing");
    let backoff = Gate::new(&sandbox, "backoff");
    let faults = format!(
        "publish.before={};backoff={}",
        publishing.action(),
        backoff.action()
    );
    let ws = workspace(&a, sandbox.origin(), &[("BEAD_FAILPOINTS", &faults)]);
    let attempts = claim_files(&ws).unwrap();
    let attempt = |op: &str| {
        attempts
            .iter()
            .find(|path| read_attempt(path, None)[0]["op"].as_str() == Some(op))
            .unwrap()
    };
    let first = attempt("claim");
    let last = attempt("release");
    let first_id = text(&read_attempt(first, None)[0]["id"]);
    let last_id = text(&read_attempt(last, None)[0]["id"]);
    assert_ne!(first_id, last_id);
    let first_pending = pending_twin(&ws.dir, first);

    let (result, met) = alongside(
        || synchronise(&ws, &Endpoint::at(sandbox.origin())),
        |done| {
            let published = publishing.meet(done, || {
                assert!(
                    !first_pending.exists(),
                    "the first merge confirmed its twin"
                );
                // Advance the remote while a holds its old head, so its push is rejected.
                b.bead(["create", "after the first merge"]);
            });
            let retried = backoff.meet(done, || {
                // The store is unlocked here; the next merge will confirm a different twin.
                pending_twin(&ws.dir, last)
            });
            (published, retried)
        },
    );
    assert!(
        met.0.is_some() && met.1.is_some(),
        "both round boundaries were reached"
    );
    let synced = result.unwrap();
    assert_eq!((synced.attempts, synced.pulled, synced.pushed), (2, 2, 1));
    assert_eq!(synced.confirmed, [last_id]);
    assert!(
        !met.1.unwrap().exists(),
        "the final merge confirmed its twin"
    );
    assert_eq!(ws.settings.failpoints.hits("publish.before"), 2);
    assert_eq!(ws.settings.failpoints.hits("backoff"), 1);
    b.bead(["sync"]);
    assert_eq!(a.export(), b.export());
}

/// A merge that renames a copy, then finds `sync.git` damaged reading a later file, still reports
/// the rename once the rebuilt round succeeds: the keys a sync reports against are kept before
/// the merge's first write. Kept as that merge ended, they were taken in the next round, after the
/// rename, which the report then left out.
#[test]
fn a_rebuilt_round_reports_the_rename_its_first_merge_made() {
    let sandbox = sandbox();
    let (mut m1, m2) = (Machine::new(&sandbox, "m1"), Machine::new(&sandbox, "m2"));
    m1.extra.insert("BEAD_ROTATE_BYTES".into(), "1".into());
    m1.bead(["init", "--prefix", "ex"]);
    m2.bead(["sync"]);
    let record = json!({
        "id": "ex-a", "title": "A", "status": "open", "priority": 2, "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
    });
    let path = sandbox.root().join("ext.jsonl");
    std::fs::write(&path, json_line(&record) + "\n").unwrap();
    let path = path.to_str().expect("a UTF-8 path");
    // m1's import and a later create, each in a segment of its own, in that order
    m1.bead(["import", path]);
    m1.bead(["create", "later"]);
    m2.bead(["import", path, "--offline"]);
    let origin = sandbox.origin().to_str().unwrap();
    let tip = git(
        ["--git-dir", origin, "rev-parse", "refs/beads/journal"],
        None,
    );
    let listed = git(
        [
            "--git-dir",
            origin,
            "ls-tree",
            "-r",
            "--name-only",
            tip.trim(),
        ],
        None,
    );
    let last = (listed.lines())
        .filter(|path| path.ends_with(".jsonl") && !path.contains("/claims/"))
        .max()
        .expect("m1's last segment")
        .to_owned();
    let blob = git(
        [
            "--git-dir",
            origin,
            "rev-parse",
            &format!("{}:{last}", tip.trim()),
        ],
        None,
    );
    // held once fetched, before the merge reads a blob: the damage is to m1's last segment, which
    // the merge reads after it wrote the segment before it
    let gate = Gate::new(&sandbox, "fetched");
    let faults = format!("fetch.fetched={};backoff@always=skip", gate.action());
    let ws = workspace(&m2, sandbox.origin(), &[("BEAD_FAILPOINTS", &faults)]);
    let (result, met) = alongside(
        || synchronise(&ws, &Endpoint::at(sandbox.origin())),
        |done| gate.meet(done, || damage_tip(&ws.dir.join("sync.git"), blob.trim())),
    );
    assert!(met.is_some(), "the sync fetched");
    let synced = result.expect("the rebuilt round succeeds");
    assert!(
        synced.sync_repo_rebuilt.is_some(),
        "the merge found sync.git damaged"
    );
    let from = |r: &Value| r.get("from").and_then(Value::as_str) == Some("ex-a");
    assert!(synced.renamed.iter().any(from), "{:?}", synced.renamed);
}
