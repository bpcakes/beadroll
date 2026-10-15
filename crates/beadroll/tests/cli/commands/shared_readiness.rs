//! The readers that judge every issue at once — `bead ready`, `bead blocked` and `bead prime`'s
//! count of what is ready — agree with `bead show`'s `blocked`, which judges one issue
//! (roll-lqjj.19), on a tracker with a prerequisite inherited down a chain and through a second
//! parent, a merged parent loop, an epic waiting for its child, and done ancestors and targets;
//! `blocked` still lists each issue's blockers, and a scope still leaves out what an outside
//! prerequisite holds.

use std::collections::BTreeSet;
use std::path::PathBuf;

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, Sandbox, json};

/// When every record was made and last changed.
const CREATED: &str = "2026-01-01T00:00:00Z";

/// A Beads record: `key`, of `kind`, in `status`, a child of each of `parents`, waiting for each of
/// `blocked_by`.
fn record(key: &str, kind: &str, status: &str, parents: &[&str], blocked_by: &[&str]) -> Json {
    let links = [("parent-child", parents), ("blocks", blocked_by)];
    let deps: Vec<Json> = links
        .into_iter()
        .flat_map(|(kind, targets)| {
            targets.iter().map(move |to| {
                json!({"issue_id": key, "depends_on_id": (*to), "type": kind,
                       "created_at": CREATED, "created_by": "seed"})
            })
        })
        .collect();
    let mut record = json!({"id": key, "title": key, "status": status, "priority": 2,
                            "issue_type": kind, "created_at": CREATED, "updated_at": CREATED,
                            "created_by": "seed"});
    if let Json::Object(members) = &mut record {
        if status == "closed" {
            members.insert("closed_at".into(), Json::from(CREATED));
        }
        if !deps.is_empty() {
            members.insert("dependencies".into(), Json::Array(deps));
        }
    }
    record
}

/// The tracker: a gate holding a chain of three beneath a root, and through the chain's end a
/// second parent's child; a second gate holding a loop and what is beneath it; an epic with an open
/// and a closed child; a closed parent with a prerequisite, and an issue waiting for a closed one.
fn tree() -> Vec<Json> {
    vec![
        record("ex-gate", "task", "open", &[], &[]),
        record("ex-root", "task", "open", &[], &["ex-gate"]),
        record("ex-c1", "task", "open", &["ex-root"], &[]),
        record("ex-c2", "task", "open", &["ex-c1"], &[]),
        record("ex-c3", "task", "open", &["ex-c2"], &[]),
        record("ex-free", "task", "open", &[], &[]),
        record("ex-s", "task", "open", &["ex-free", "ex-c3"], &[]),
        record("ex-gate2", "task", "open", &[], &[]),
        record("ex-k", "task", "open", &["ex-m"], &[]),
        record("ex-m", "task", "open", &["ex-k"], &["ex-gate2"]),
        record("ex-z", "task", "open", &["ex-m"], &[]),
        record("ex-e", "epic", "open", &[], &[]),
        record("ex-x", "task", "open", &["ex-e"], &[]),
        record("ex-y", "task", "closed", &["ex-e"], &[]),
        record("ex-done", "task", "closed", &[], &["ex-gate"]),
        record("ex-d1", "task", "open", &["ex-done"], &[]),
        record("ex-t", "task", "open", &[], &["ex-done"]),
    ]
}

/// `rows` as JSON lines in a file at the sandbox's root.
fn records(sandbox: &Sandbox, rows: &[Json]) -> PathBuf {
    let path = sandbox.root().join("records.jsonl");
    let text: String = rows
        .iter()
        .map(|row| write(row, Format::ASCII).unwrap() + "\n")
        .collect();
    std::fs::write(&path, text).unwrap();
    path
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

/// What `bead blocked` lists: each issue's key and the keys of its blockers, in its order.
fn blocked(machine: &Machine) -> Vec<(String, Vec<String>)> {
    let listed = machine.bead(["blocked"]).unwrap();
    (listed.as_array().unwrap().iter())
        .map(|item| {
            let blockers = (item["blocked_by"].as_array().unwrap().iter())
                .map(|key| key.as_str().unwrap().to_owned())
                .collect();
            (item["id"].as_str().unwrap().to_owned(), blockers)
        })
        .collect()
}

/// Every reader against `show`: `ready` lists exactly the open, unheld issues; `blocked` lists
/// exactly the open issues whose detail says `blocked`; `prime` counts what `ready` lists.
fn readers_agree(machine: &Machine, keys: &[&str], ready: &[&str]) {
    let shown: Vec<(String, bool, bool)> = (keys.iter())
        .map(|key| {
            let detail = machine.show(key);
            let open = detail["status"].as_str() == Some("open");
            (
                (*key).to_owned(),
                open,
                detail["blocked"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(machine.ready(), set(ready));
    let unheld_open: BTreeSet<String> = (shown.iter())
        .filter(|(_, open, held)| *open && !held)
        .map(|(key, _, _)| key.clone())
        .collect();
    assert_eq!(
        unheld_open,
        set(ready),
        "ready is what show calls open and not blocked"
    );
    let mut held_open: Vec<String> = (shown.iter())
        .filter(|(_, open, held)| *open && *held)
        .map(|(key, _, _)| key.clone())
        .collect();
    held_open.sort();
    let listed: Vec<String> = blocked(machine).into_iter().map(|(key, _)| key).collect();
    assert_eq!(
        listed, held_open,
        "blocked is what show calls open and blocked, by key"
    );
    let prime = machine.run(["prime"]).stdout;
    let wanted = format!("Tracker 'ex': {} ready", ready.len());
    assert!(prime.contains(&wanted), "{wanted:?} in {prime}");
}

#[test]
fn ready_blocked_and_prime_judge_every_issue_at_once_as_show_judges_each() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let path = records(&sandbox, &tree());
    a.bead(["init", "--prefix", "ex", "--import", path.to_str().unwrap()]);
    let keys: Vec<&str> = [
        "ex-gate", "ex-root", "ex-c1", "ex-c2", "ex-c3", "ex-free", "ex-s", "ex-gate2", "ex-k",
        "ex-m", "ex-z", "ex-e", "ex-x", "ex-y", "ex-done", "ex-d1", "ex-t",
    ]
    .to_vec();
    readers_agree(
        &a,
        &keys,
        &["ex-gate", "ex-gate2", "ex-free", "ex-x", "ex-d1", "ex-t"],
    );
    // the lists are what they were: inherited down the chain and through the second parent, around
    // the loop, and an epic's open child; a done issue's own prerequisite still shows on it
    let lists: Vec<(&str, Vec<&str>)> = [
        ("ex-c1", vec!["ex-gate"]),
        ("ex-c2", vec!["ex-gate"]),
        ("ex-c3", vec!["ex-gate"]),
        ("ex-e", vec!["ex-x"]),
        ("ex-k", vec!["ex-gate2"]),
        ("ex-m", vec!["ex-gate2"]),
        ("ex-root", vec!["ex-gate"]),
        ("ex-s", vec!["ex-gate"]),
        ("ex-z", vec!["ex-gate2"]),
    ]
    .to_vec();
    let wanted: Vec<(String, Vec<String>)> = (lists.into_iter())
        .map(|(key, blockers)| {
            (
                key.to_owned(),
                blockers.into_iter().map(str::to_owned).collect(),
            )
        })
        .collect();
    assert_eq!(blocked(&a), wanted);
    assert_eq!(a.show("ex-done")["blocked"].as_bool(), Some(true));
    assert_eq!(a.show("ex-y")["blocked"].as_bool(), Some(false));
    // the scope leaves out what the gate outside it holds, and lists it once the gate closes
    assert_eq!(a.bead(["ready", "ex-root"]), Some(Json::Array(Vec::new())));
    a.bead(["close", "ex-gate", "--reason", "done"]);
    readers_agree(
        &a,
        &keys,
        &[
            "ex-gate2", "ex-free", "ex-x", "ex-d1", "ex-t", "ex-root", "ex-c1", "ex-c2", "ex-c3",
            "ex-s",
        ],
    );
    let scoped = a.bead(["ready", "ex-root"]).unwrap();
    let scoped: Vec<&str> = (scoped.as_array().unwrap().iter())
        .map(|issue| issue["id"].as_str().unwrap())
        .collect();
    assert_eq!(scoped, ["ex-c1", "ex-c2", "ex-c3", "ex-s"]);
    assert_eq!(
        blocked(&a)
            .into_iter()
            .map(|(key, _)| key)
            .collect::<Vec<_>>(),
        ["ex-e", "ex-k", "ex-m", "ex-z"]
    );
}
