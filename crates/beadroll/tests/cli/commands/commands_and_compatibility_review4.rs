//! `CommandsAndCompatibilityReview4`, from the fourth independent review: no claim outlives a
//! tombstone, an offline close ends the provisional claims it saw, a split import equals a
//! one-shot one, and an imported field cannot poison dependency bookkeeping.

use std::path::{Path, PathBuf};

use beadroll_core::json::{Format, Object, write};
use beadroll_test_support::{Json, Machine, code, json, parse_json};

fn id(value: &Json) -> String {
    value["id"].as_str().unwrap().to_owned()
}

/// Member `key` of `value`; absence fails the test.
fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}

/// Spaced, ASCII-escaped JSON.
fn dumps(value: &Json) -> String {
    write(value, Format::ASCII).unwrap()
}

/// The entries of `dir` whose names end in `suffix`: none when
/// `dir` is not a directory.
fn glob(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_str().unwrap().ends_with(suffix))
        .collect()
}

/// Q05: a terminal tombstone transition must not survive with a live claim.
#[test]
fn q05_tombstone_update_cannot_leave_a_releasable_claim() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q05-tombstone");
    a.bead(["init", "--prefix", "ex"]);
    let key = id(&a.bead(["create", "delete me"]).unwrap());
    a.bead(["sync"]);
    let result = a.run(["update", &key, "--claim", "--status", "tombstone", "--json"]);
    if result.returncode() != 0 {
        assert_eq!(code(&result), "VALIDATION");
        let shown = a.show(&key);
        assert_eq!(shown["status"], json!("open"));
        assert!(shown.get("claim").is_none(), "{shown:?}");
        assert_eq!(
            glob(&a.store().join("pending"), ".json"),
            Vec::<PathBuf>::new()
        );
        // events/*/claims/*.json
        let claims: Vec<PathBuf> = glob(&a.store().join("events"), "")
            .iter()
            .flat_map(|replica| glob(&replica.join("claims"), ".json"))
            .collect();
        assert_eq!(claims, Vec::<PathBuf>::new());
        let peer = Machine::new(&sandbox, "q05-rejected-peer");
        peer.bead(["sync"]);
        assert_eq!(peer.show(&key)["status"], json!("open"));
        assert!(peer.show(&key).get("claim").is_none());
        return; // rejecting this spelling in favor of `delete` is safe
    }
    let shown = a.show(&key);
    assert_eq!(shown["status"], json!("tombstone"));
    let released = a.run(["release", &key, "--json"]);
    assert_ne!(
        released.returncode(),
        0,
        "release resurrected a tombstoned issue"
    );
    assert_eq!(a.show(&key)["status"], json!("tombstone"));
    assert!(a.show(&key).get("claim").is_none());
}

/// Q11: close clears seen provisional claims but preserves concurrent ones.
#[test]
fn q11_offline_close_ends_a_provisional_claim() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q11-provisional");
    a.bead(["init", "--prefix", "ex"]);
    let key = id(&a.bead(["create", "offline work"]).unwrap());
    a.bead(["sync"]);
    let concurrent = Machine::new(&sandbox, "q11-concurrent");
    concurrent.bead(["sync"]);

    // This claim has an earlier id but was made from a stale frontier, so A's later close did not
    // see it and must not erase it.
    concurrent.bead(["claim", &key, "--offline"]);
    a.bead(["claim", &key, "--offline"]);
    a.bead(["close", &key, "--offline"]);
    a.bead(["sync"]);
    concurrent.bead(["sync"]);
    a.bead(["sync"]);
    let shown = a.show(&key);
    assert_eq!(shown["status"], json!("closed"));
    let provisional = shown
        .get("provisional_claims")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let provisional = provisional.as_array().unwrap();
    assert_eq!(provisional.len(), 1, "{provisional:?}");
    let owner = provisional[0]["owner"].as_str().unwrap();
    assert!(owner.contains("q11-concurrent-thread"), "{owner}");
    let claims = a.bead(["claims"]).unwrap()["claims"].clone();
    let claims = claims.as_array().unwrap();
    assert_eq!(claims.len(), 1, "{claims:?}");
    let owner = claims[0]["owner"].as_str().unwrap();
    assert!(owner.contains("q11-concurrent-thread"), "{owner}");
}

/// An issue record as Q06 writes it: `id` and `title`, then the fields both of its records share.
fn q06_issue(id: &str, title: &str) -> Object {
    let record = json!({
        "id": id, "title": title, "status": "open", "priority": 2, "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed"
    });
    record.as_object().unwrap().clone()
}

/// Q06: split and one-shot imports must have the same dependencies and readiness.
#[test]
fn q06_incremental_import_resolves_late_dependency_target() {
    let sandbox = crate::sandbox();
    let dep = json!({
        "issue_id": "ex-a", "depends_on_id": "ex-b", "type": "blocks",
        "created_at": "2026-01-01T00:00:00Z", "created_by": "seed",
        "metadata": "{}", "thread_id": ""
    });
    let mut issue_a = q06_issue("ex-a", "A");
    issue_a.insert("dependencies".into(), json!([dep]));
    let (issue_a, issue_b) = (Json::from(issue_a), Json::from(q06_issue("ex-b", "B")));
    let root = sandbox.root();
    let (a_path, b_path, all_path) = (
        root.join("q06-a.jsonl"),
        root.join("q06-b.jsonl"),
        root.join("q06-all.jsonl"),
    );
    std::fs::write(&a_path, dumps(&issue_a) + "\n").unwrap();
    std::fs::write(&b_path, dumps(&issue_b) + "\n").unwrap();
    let both = dumps(&issue_a) + "\n" + &dumps(&issue_b) + "\n";
    std::fs::write(&all_path, both).unwrap();

    let split = Machine::new(&sandbox, "q06-split");
    split.bead(["init", "--prefix", "ex"]);
    split.bead(["import", a_path.to_str().unwrap()]);
    split.bead(["import", b_path.to_str().unwrap()]);
    let one = Machine::new(&sandbox, "q06-one");
    one.bead(["init", "--prefix", "ex"]);
    one.bead(["import", all_path.to_str().unwrap()]);

    assert_eq!(
        split.export(),
        one.export(),
        "interchange data should already agree"
    );
    let (split_a, one_a) = (split.show("ex-a"), one.show("ex-a"));
    assert_eq!(
        member(&split_a, "dependencies"),
        member(&one_a, "dependencies")
    );
    assert_eq!(member(&split_a, "blocked"), member(&one_a, "blocked"));
    assert_eq!(split.ready(), one.ready());
}

/// Q10: an imported unknown field must not poison internal dependency bookkeeping.
#[test]
fn q10_reserved_dangling_extra_cannot_crash_export() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q10-import");
    a.bead(["init", "--prefix", "ex"]);
    let record = json!({
        "id": "ex-one", "title": "one", "status": "open", "priority": 2,
        "issue_type": "task", "_dangling_dependencies": "user supplied"
    });
    let path = sandbox.root().join("q10.jsonl");
    std::fs::write(&path, dumps(&record) + "\n").unwrap();
    a.bead(["import", path.to_str().unwrap()]);
    let exported = a.run(["export"]);
    assert_eq!(exported.returncode(), 0, "{}", exported.stderr);
    let exported = parse_json(&exported.stdout).unwrap();
    assert_eq!(exported["_dangling_dependencies"], json!("user supplied"));
}
