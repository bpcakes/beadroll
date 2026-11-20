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
/// rename, which the report then left out. (AT101)
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

/// A round that wrote nothing and was contended keeps the keys it began with: another process's
/// sync during the backoff renames m2's copy, the retry then writes nothing, and the report says the
/// copy moved from the key m2 had when its sync began. The keys kept only by a merge that wrote,
/// the retry kept none and reported nothing. (AT101)
#[test]
fn a_retried_round_reports_against_the_keys_the_sync_began_with() {
    let sandbox = sandbox();
    let (m1, m2, m3) = (
        Machine::new(&sandbox, "m1"),
        Machine::new(&sandbox, "m2"),
        Machine::new(&sandbox, "m3"),
    );
    m1.bead(["init", "--prefix", "ex"]);
    m2.bead(["sync"]);
    m3.bead(["sync"]);
    let record = json!({
        "id": "ex-a", "title": "A", "status": "open", "priority": 2, "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
    });
    let path = sandbox.root().join("ext.jsonl");
    std::fs::write(&path, json_line(&record) + "\n").unwrap();
    let path = path.to_str().expect("a UTF-8 path");
    // m3's copy is made first, so m2's gives way once both are merged
    m3.bead(["import", path, "--offline"]);
    m2.bead(["import", path, "--offline"]);
    let publishing = Gate::new(&sandbox, "publishing");
    let backoff = Gate::new(&sandbox, "backoff");
    let faults = format!(
        "publish.before={};backoff={}",
        publishing.action(),
        backoff.action()
    );
    let ws = workspace(&m2, sandbox.origin(), &[("BEAD_FAILPOINTS", &faults)]);
    let (result, met) = alongside(
        || synchronise(&ws, &Endpoint::at(sandbox.origin())),
        |done| {
            // m3 publishes while m2 holds its old head, so m2's push is rejected
            let published = publishing.meet(done, || m3.bead(["sync"]));
            // the store is unlocked: another process's sync merges m3's copy, renaming m2's
            let retried = backoff.meet(done, || m2.bead(["sync"]));
            (published, retried)
        },
    );
    assert!(
        met.0.is_some() && met.1.is_some(),
        "both round boundaries were reached"
    );
    let synced = result.expect("the retried round succeeds");
    let from = |r: &Value| r.get("from").and_then(Value::as_str) == Some("ex-a");
    assert!(synced.renamed.iter().any(from), "{:?}", synced.renamed);
}

/// A merge that fails before it writes (`sync.git` damaged at the first blob it reads) keeps the
/// keys the sync began with for the round the rebuild retries: another process's sync before that
/// round renames m2's copy, the retry then writes nothing, and the report says the copy moved.
/// Kept only by a write or by a contended round, the keys were none, and the report empty. (AT101)
#[test]
fn a_merge_that_fails_before_writing_keeps_the_keys_the_sync_began_with() {
    let sandbox = sandbox();
    let (m1, m2, m3) = (
        Machine::new(&sandbox, "m1"),
        Machine::new(&sandbox, "m2"),
        Machine::new(&sandbox, "m3"),
    );
    m1.bead(["init", "--prefix", "ex"]);
    m2.bead(["sync"]);
    m3.bead(["sync"]);
    let record = json!({
        "id": "ex-a", "title": "A", "status": "open", "priority": 2, "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
    });
    let path = sandbox.root().join("ext.jsonl");
    std::fs::write(&path, json_line(&record) + "\n").unwrap();
    let path = path.to_str().expect("a UTF-8 path");
    // m3's copy is made first and published; m2's gives way once both are merged
    m3.bead(["import", path]);
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
    let ours = |path: &&str| path.ends_with(".jsonl") && !path.contains("/claims/");
    let segments: Vec<&str> = listed.lines().filter(ours).collect();
    let ws = workspace(&m2, sandbox.origin(), &[]);
    let sync_git = ws.dir.join("sync.git");
    git(
        [
            "--git-dir",
            sync_git.to_str().unwrap(),
            "fetch",
            "-q",
            origin,
            "+refs/beads/journal:refs/beads/held",
        ],
        None,
    );
    // the blob of the segment m2 has not merged, m3's import, damaged before the merge reads it
    let m3_segment = (segments.iter())
        .find(|path| !ws.dir.join(path).exists())
        .expect("a segment m2 lacks");
    let blob = git(
        [
            "--git-dir",
            origin,
            "rev-parse",
            &format!("{}:{m3_segment}", tip.trim()),
        ],
        None,
    );
    damage_tip(&sync_git, blob.trim());
    let retry = Gate::new(&sandbox, "retry");
    let faults = format!(
        "fetch.fetched@nth(2)={};backoff@always=skip",
        retry.action()
    );
    let ws = workspace(&m2, sandbox.origin(), &[("BEAD_FAILPOINTS", &faults)]);
    let (result, met) = alongside(
        || synchronise(&ws, &Endpoint::at(sandbox.origin())),
        // between the rebuild and the retry's merge, another process syncs the store
        |done| retry.meet(done, || m2.bead(["sync"])),
    );
    assert!(met.is_some(), "the rebuilt round fetched");
    let synced = result.expect("the rebuilt round succeeds");
    assert!(
        synced.sync_repo_rebuilt.is_some(),
        "the first merge found sync.git damaged"
    );
    let from = |r: &Value| r.get("from").and_then(Value::as_str) == Some("ex-a");
    assert!(synced.renamed.iter().any(from), "{:?}", synced.renamed);
}

/// A key another process restores between a sync's rounds, with no rename on either side, is
/// reported: b's copy of a's journal gives an issue a rewritten key, b's merge finds the published
/// copy damaged before it puts it back, and another sync of b's store puts it back before the
/// rebuilt round, which writes nothing. With no rename before or after, the report compared no key
/// unless the round that gave way had marked it (AT101).
#[test]
fn a_key_restored_between_rounds_without_a_rename_is_reported() {
    let sandbox = sandbox();
    let (a, b) = (Machine::new(&sandbox, "a"), Machine::new(&sandbox, "b"));
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "the issue"]).unwrap()["id"]);
    b.bead(["sync"]);
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
    let ws = workspace(&b, sandbox.origin(), &[]);
    // a's segment, as b holds it, with the create's key rewritten
    let segment = (listed.lines())
        .filter(|path| path.ends_with(".jsonl") && !path.contains("/claims/"))
        .find(|path| std::fs::read_to_string(ws.dir.join(path)).is_ok_and(|t| t.contains(&key)))
        .expect("a's segment")
        .to_owned();
    let forged = format!("{key}-forged");
    let held = std::fs::read_to_string(ws.dir.join(&segment)).unwrap();
    let rewritten = held.replace(
        &format!("\"key\":\"{key}\""),
        &format!("\"key\":\"{forged}\""),
    );
    assert_ne!(rewritten, held, "the create names the key");
    std::fs::write(ws.dir.join(&segment), rewritten).unwrap();
    assert_eq!(text(&b.show(&forged)["title"]), "the issue");
    // the published copy, damaged in b's sync.git before the merge reads it
    let sync_git = ws.dir.join("sync.git");
    let fetch = [
        "--git-dir",
        sync_git.to_str().unwrap(),
        "fetch",
        "-q",
        origin,
        "+refs/beads/journal:refs/beads/held",
    ];
    git(fetch, None);
    let blob = git(
        [
            "--git-dir",
            origin,
            "rev-parse",
            &format!("{}:{segment}", tip.trim()),
        ],
        None,
    );
    damage_tip(&sync_git, blob.trim());
    let retry = Gate::new(&sandbox, "retry");
    let faults = format!(
        "fetch.fetched@nth(2)={};backoff@always=skip",
        retry.action()
    );
    let ws = workspace(&b, sandbox.origin(), &[("BEAD_FAILPOINTS", &faults)]);
    let (result, met) = alongside(
        || synchronise(&ws, &Endpoint::at(sandbox.origin())),
        |done| retry.meet(done, || b.bead(["sync"])),
    );
    assert!(met.is_some(), "the rebuilt round fetched");
    let synced = result.expect("the rebuilt round succeeds");
    assert!(
        synced.sync_repo_rebuilt.is_some(),
        "the first merge found sync.git damaged"
    );
    let back = |r: &Value| {
        r.get("from").and_then(Value::as_str) == Some(forged.as_str())
            && r.get("to").and_then(Value::as_str) == Some(key.as_str())
    };
    assert!(synced.renamed.iter().any(back), "{:?}", synced.renamed);
}
