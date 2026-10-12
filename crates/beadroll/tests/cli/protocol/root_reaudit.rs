//! `RootReaudit`: the second independent audit's command-level checks
//! (`docs/reviews/2026-10-06-2-reaudit.md`): a local import keeps the ids of the comments already
//! exported (R13); a forced close through `update` publishes as `close` does (R12); and a claiming
//! update that fails writes, claims and publishes nothing, whichever of its targets it fails on
//! (I06).

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, Sandbox, code, json, parse_json};

/// `setUp`: machine `root2-a` has initialised tracker `re`, created the issue every test works on
/// and synced. Returns the machine and the issue's id.
fn set_up(sandbox: &Sandbox) -> (Machine, String) {
    let a = Machine::new(sandbox, "root2-a");
    a.bead(["init", "--prefix", "re"]);
    let created = a.bead(["create", "root re-audit"]).expect("the issue");
    let key = created["id"].as_str().expect("an id").to_owned();
    a.bead(["sync"]);
    (a, key)
}

/// The first comment id in the exported record for key.
fn first_comment_id(export: &str, key: &str) -> Json {
    let record = export
        .lines()
        .map(|line| parse_json(line).unwrap_or_else(|error| panic!("not JSON ({error}): {line}")))
        .find(|record| record.get("id").expect("a record's id").as_str() == Some(key))
        .unwrap_or_else(|| panic!("no record of {key} in {export}"));
    let comment = &record["comments"][0];
    let id = comment.get("id").cloned();
    id.unwrap_or_else(|| panic!("no first comment id in {record:?}"))
}

/// `bead claims`' list of claims on `machine`.
fn claims(machine: &Machine) -> Json {
    let listed = machine.bead(["claims"]).expect("the claims");
    listed["claims"].clone()
}

/// R13: importing a legacy Beads record whose comment has id 41 does not renumber the comment this
/// machine had already exported.
#[test]
fn importing_legacy_comment_does_not_renumber_existing_local_comment() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    a.bead(["comments", "add", &key, "existing local comment"]);
    let before = first_comment_id(&a.export(), &key);
    let src = sandbox.root().join("legacy.jsonl");
    let legacy = json!({
        "id": "re-legacy",
        "title": "legacy issue",
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "audit",
        "comments": [{
            "id": 41,
            "issue_id": "re-legacy",
            "author": "audit",
            "text": "legacy",
            "created_at": "2026-01-01T00:00:00Z"
        }]
    });
    let line = write(&legacy, Format::ASCII).expect("a record") + "\n";
    std::fs::write(&src, line).expect("the legacy file");
    a.bead(["import", src.to_str().expect("a UTF-8 path")]);
    let after = first_comment_id(&a.export(), &key);
    assert_eq!(
        after, before,
        "single-machine import renumbered an already exported comment"
    );
}

/// R12: a machine that does not hold the claim force-closes the issue through
/// `update --status closed --force`: the claim ends, and the close is published at once, as
/// `close --force` publishes it, so the owner sees the issue closed after a sync.
#[test]
fn forced_close_through_update_publishes_like_forced_close() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    a.bead(["claim", &key]);
    let b = Machine::new(&sandbox, "root2-b");
    b.bead(["sync"]);
    let close = ["update", &key, "--status", "closed", "--force"];
    let updated = b.bead(close).expect("the updated issue");
    let result = &updated[0];
    assert_eq!(result["status"].as_str(), Some("closed"));
    assert!(result.get("claim").is_none(), "{result:?}");
    a.bead(["sync"]);
    assert_eq!(
        a.show(&key)["status"].as_str(),
        Some("closed"),
        "forced update close was never published"
    );
}

/// I06: a claiming update of an issue and its child that would make the child the parent of both
/// fails with `CYCLE` before anything is written: the export is unchanged, and nothing is claimed,
/// here or on a machine that syncs afterwards.
#[test]
fn claim_update_cycle_validation_is_atomic_for_all_targets() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    let created = a.bead(["create", "child", "--parent", &key]);
    let child = created.expect("the child")["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let before = a.export();
    let proc = a.run([
        "update",
        &key,
        &child,
        "--claim",
        "--parent",
        &child,
        "--title",
        "must not persist",
        "--json",
    ]);
    assert_eq!(proc.returncode(), 1, "{}", proc.stderr);
    assert_eq!(code(&proc), "CYCLE");
    assert_eq!(a.export(), before);
    assert_eq!(claims(&a), Json::Array(Vec::new()));
    let c = Machine::new(&sandbox, "root2-c");
    c.bead(["sync"]);
    assert_eq!(claims(&c), Json::Array(Vec::new()));
}

/// A claiming update refused because another machine holds the claim (`ALREADY_CLAIMED`) publishes
/// none of the field edits bundled with it: once both machines have synced, both show the original
/// title.
#[test]
fn failed_claim_does_not_publish_bundled_field_edit() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    a.bead(["claim", &key]);
    let b = Machine::new(&sandbox, "root2-b");
    b.bead(["sync"]);
    let proc = b.run([
        "update",
        &key,
        "--claim",
        "--title",
        "must not persist",
        "--json",
    ]);
    assert_eq!(code(&proc), "ALREADY_CLAIMED");
    b.bead(["sync"]);
    a.bead(["sync"]);
    assert_eq!(a.show(&key)["title"].as_str(), Some("root re-audit"));
    assert_eq!(b.show(&key)["title"].as_str(), Some("root re-audit"));
}
