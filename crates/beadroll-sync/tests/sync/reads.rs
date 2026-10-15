//! How often a sync and a write read the state of the store's journal (roll-lqjj.29): once per
//! round of a sync, by its merge, which the report and an ownership command's transaction go on
//! from, and once for a write's transaction. Counted at the `projection.read` failpoint.

use beadroll_core::json::{Format, Object, Value, write};
use beadroll_store::{Endpoint, Workspace};
use beadroll_sync::{publish_soon, synchronise, writing};
use beadroll_test_support::{Machine, json};

use crate::support::{actor, claim_command, sandbox, text, workspace};

/// A spec that never fires and counts every read.
const COUNTED: (&str, &str) = ("BEAD_FAILPOINTS", "projection.read@nth(1000000)=sleep(0)");

/// How often `ws` has read its store's state.
fn reads(ws: &Workspace) -> u64 {
    ws.settings.failpoints.hits("projection.read")
}

/// A sync that brings nothing in reads the state once, and reports it as before.
#[test]
fn a_sync_that_brings_nothing_reads_the_state_once() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    a.bead(["create", "y", "--offline"]);
    let ws = workspace(&a, sandbox.origin(), &[COUNTED]);
    let synced = synchronise(&ws, &Endpoint::at(sandbox.origin())).expect("a sync");
    assert_eq!(reads(&ws), 1);
    assert_eq!((synced.pulled, synced.pushed, synced.issues), (0, 1, 2));
    assert!(synced.renamed.is_empty(), "{:?}", synced.renamed);
    let again = synchronise(&ws, &Endpoint::at(sandbox.origin())).expect("a sync");
    assert_eq!(reads(&ws), 2);
    assert_eq!((again.pulled, again.pushed, again.issues), (0, 0, 2));
}

/// A sync that brings records in reads the state before it merges, to count the keys renamed
/// before it, and after: the copy the merge renamed is reported, and only it.
#[test]
fn a_sync_that_brings_a_rename_reads_the_state_before_and_after_its_merge() {
    let sandbox = sandbox();
    let record = json!({
        "id": "ex-a",
        "title": "A",
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed",
    });
    let path = sandbox.root().join("ext.jsonl");
    let line = write(&record, Format::ASCII).expect("a record") + "\n";
    std::fs::write(&path, line).unwrap();
    let path = path.to_str().expect("a UTF-8 path");
    let (m1, m2) = (Machine::new(&sandbox, "m1"), Machine::new(&sandbox, "m2"));
    m1.bead(["init", "--prefix", "ex"]);
    m2.bead(["sync"]);
    m1.bead(["import", path]);
    m2.bead(["import", path, "--offline"]);
    let ws = workspace(&m2, sandbox.origin(), &[COUNTED]);
    let synced = synchronise(&ws, &Endpoint::at(sandbox.origin())).expect("a sync");
    assert_eq!(reads(&ws), 2);
    assert_eq!((synced.pulled, synced.issues), (1, 2));
    assert_eq!(synced.renamed.len(), 1, "{:?}", synced.renamed);
    let renamed = &synced.renamed[0];
    assert_eq!(renamed.get("from").and_then(Value::as_str), Some("ex-a"));
    // the next sync brings nothing: nothing more is renamed since it began
    let again = synchronise(&ws, &Endpoint::at(sandbox.origin())).expect("a sync");
    assert!(again.renamed.is_empty(), "{:?}", again.renamed);
    assert_eq!(reads(&ws), 3);
}

/// A write published at once reads the state for its transaction, then once for the merge of the
/// sync that publishes it, which makes no report.
#[test]
fn a_published_write_reads_the_state_for_its_transaction_and_its_merge() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "x"]).expect("an issue")["id"]);
    let ws = workspace(&a, sandbox.origin(), &[COUNTED]);
    let actor = actor(&a);
    writing(&ws, &actor, |tx| {
        let uid = tx.state()?.resolve(&key).expect("the issue").uid();
        let mut data = Object::new();
        data.insert("text".into(), Value::from("noted"));
        tx.emit("comment", Some(&uid.to_string()), data, None)
    })
    .expect("a comment");
    assert_eq!(reads(&ws), 1);
    assert_eq!(publish_soon(&ws, false, None), None);
    assert_eq!(reads(&ws), 2);
    let shown = a.show(&key);
    assert_eq!(shown["comments"][0]["text"].as_str(), Some("noted"));
}

/// An ownership command's round reads the state once, by its merge, which its transaction starts
/// from.
#[test]
fn a_claim_reads_the_state_once_a_round() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "x"]).expect("an issue")["id"]);
    let ws = workspace(&a, sandbox.origin(), &[COUNTED]);
    claim_command(&ws, &actor(&a), &key).expect("a claim");
    assert_eq!(reads(&ws), 1);
    assert!(a.show(&key).get("claim").is_some());
}
