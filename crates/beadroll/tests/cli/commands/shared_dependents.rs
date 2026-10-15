//! The commands that answer with several issues in full — `bead show`, `bead update`, `bead claim`
//! and `bead takeover` — find the issues that depend on them once for all of them (roll-lqjj.20),
//! and each detail is still the one `bead show` gives of its issue alone, member for member, in
//! the order the issues were named, an issue named twice included; a key that names no issue still
//! fails the command with ISSUE_NOT_FOUND and prints nothing else.

use std::path::PathBuf;

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, Sandbox, code, json};

/// When every record was made and last changed.
const CREATED: &str = "2026-01-01T00:00:00Z";

/// A Beads record: `key`, in `status`, with a dependency of each type on each key of `deps`.
fn record(key: &str, status: &str, deps: &[(&str, &str)]) -> Json {
    let deps: Vec<Json> = (deps.iter())
        .map(|(to, kind)| {
            json!({"issue_id": key, "depends_on_id": (*to), "type": (*kind),
                   "created_at": CREATED, "created_by": "seed"})
        })
        .collect();
    let mut record = json!({"id": key, "title": key, "status": status, "priority": 2,
                            "issue_type": "task", "created_at": CREATED, "updated_at": CREATED,
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

/// The tracker: two targets, a source with three types of edge to one and a parent-child edge to
/// the other, a closed source, a source made after the targets' key order would put it, and an
/// issue waiting for a key no issue has.
fn tracker() -> Vec<Json> {
    vec![
        record("ex-zeta", "open", &[]),
        record("ex-alpha", "open", &[]),
        record(
            "ex-src",
            "open",
            &[
                ("ex-zeta", "related"),
                ("ex-alpha", "parent-child"),
                ("ex-zeta", "blocks"),
                ("ex-zeta", "discovered-from"),
            ],
        ),
        record("ex-done", "closed", &[("ex-zeta", "blocks")]),
        record(
            "ex-b",
            "open",
            &[("ex-zeta", "related"), ("ex-alpha", "blocks")],
        ),
        record("ex-wait", "open", &[("ex-never", "blocks")]),
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

/// A value as written, members in their order: `Json` equality does not see the order.
fn text(value: &Json) -> String {
    write(value, Format::COMPACT).unwrap()
}

/// What `bead show` gives of each of `keys` alone, in order, as one list.
fn alone(machine: &Machine, keys: &[&str]) -> String {
    let shown: Vec<Json> = keys.iter().map(|key| machine.show(key)).collect();
    text(&Json::Array(shown))
}

/// The machine with the tracker imported.
fn machine(sandbox: &Sandbox) -> Machine {
    let a = Machine::new(sandbox, "a");
    let path = records(sandbox, &tracker());
    a.bead(["init", "--prefix", "ex", "--import", path.to_str().unwrap()]);
    a
}

/// The keys and types of an issue's dependents, as its detail lists them.
fn dependents(detail: &Json) -> Vec<String> {
    (detail["dependents"].as_array().unwrap().iter())
        .map(|dep| {
            let part = |name: &str| dep[name].as_str().unwrap().to_owned();
            format!("{} {}", part("id"), part("dependency_type"))
        })
        .collect()
}

#[test]
fn show_update_claim_and_takeover_answer_with_each_issue_as_show_gives_it_alone() {
    let sandbox = crate::sandbox();
    let a = machine(&sandbox);
    let keys = [
        "ex-zeta", "ex-alpha", "ex-zeta", "ex-src", "ex-done", "ex-wait",
    ];
    let shown = a.bead(["show"].into_iter().chain(keys)).unwrap();
    assert_eq!(text(&shown), alone(&a, &keys));
    // replay order, then each source's edges by type
    assert_eq!(
        dependents(&shown[0]),
        [
            "ex-src blocks",
            "ex-src discovered-from",
            "ex-src related",
            "ex-done blocks",
            "ex-b related",
        ]
    );
    assert_eq!(
        dependents(&shown[1]),
        ["ex-src parent-child", "ex-b blocks"]
    );
    assert!(dependents(&shown[3]).is_empty());

    let updated = a
        .bead([
            "update",
            "ex-alpha",
            "ex-zeta",
            "ex-alpha",
            "--priority",
            "1",
        ])
        .unwrap();
    assert_eq!(
        text(&updated),
        alone(&a, &["ex-alpha", "ex-zeta", "ex-alpha"])
    );

    let claimed = a.bead(["claim", "ex-zeta", "ex-src", "ex-zeta"]).unwrap();
    assert_eq!(text(&claimed), alone(&a, &["ex-zeta", "ex-src", "ex-zeta"]));

    let taken = a
        .cmd([
            "takeover",
            "ex-src",
            "ex-zeta",
            "--reason",
            "the work moved",
        ])
        .thread("successor")
        .json()
        .unwrap();
    assert_eq!(text(&taken), alone(&a, &["ex-src", "ex-zeta"]));
    // the successor holds what it took over
    for item in taken.as_array().unwrap() {
        let owner = item["claim"]["owner"].as_str().unwrap();
        assert!(owner.contains(":successor@"), "{owner}");
    }
}

#[test]
fn a_key_that_names_no_issue_fails_the_command_wherever_it_stands() {
    let sandbox = crate::sandbox();
    let a = machine(&sandbox);
    for args in [
        &["show", "ex-zeta", "ex-nope", "ex-alpha"][..],
        &["show", "ex-nope", "ex-zeta"],
        &["show", "ex-zeta", "ex-alpha", "ex-nope"],
        &["update", "ex-zeta", "ex-nope", "--priority", "1"],
        &["claim", "ex-alpha", "ex-nope"],
        &["takeover", "ex-nope", "ex-alpha", "--reason", "gone"],
    ] {
        let run = a.cmd(args.iter().chain(&["--json"])).run();
        assert_eq!(run.returncode(), 1, "{args:?}: {}", run.stderr);
        assert_eq!(code(&run), "ISSUE_NOT_FOUND", "{args:?}");
        assert!(
            run.stderr.contains("Issue not found: ex-nope"),
            "{args:?}: {}",
            run.stderr
        );
        assert!(run.stdout.is_empty(), "{args:?}: {}", run.stdout);
    }
    // nothing was written
    for key in ["ex-zeta", "ex-alpha"] {
        let shown = a.show(key);
        assert_eq!(shown["priority"].as_i64(), Some(2), "{key}");
        assert!(shown.get("claim").is_none(), "{key}");
    }
}
