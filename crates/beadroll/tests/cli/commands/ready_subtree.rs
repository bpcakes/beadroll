//! `ReadySubtree`: `bead ready ID`, and `--epic ID` as in br, is the frontier inside ID's subtree
//! (J01): every issue beneath ID through parent-child edges, at any depth and whatever lies in
//! between, with readiness judged on the whole tracker. `--parent ID` stays ID's direct children.
//! A scope is resolved before anything is filtered (J02), and one scope is taken at a time (J03).

use std::collections::BTreeSet;
use std::path::PathBuf;

use beadroll_core::json::{Format, Object, write};
use beadroll_test_support::{Json, Machine, Sandbox, code, json};

/// When a record was made and last changed, unless it says otherwise (`COMMON`).
const CREATED: &str = "2026-01-01T00:00:00Z";

/// ex-e's ready descendants, in ready's order.
const SUBTREE: [&str; 6] = ["ex-d1", "ex-t", "ex-f", "ex-s", "ex-c1", "ex-f1"];

/// A Beads record as `tree_record` builds it: a child of each of its parents, waiting for each of
/// its blockers, its own fields over the common ones.
struct Record {
    key: String,
    fields: Object,
    parents: Vec<String>,
    blocked_by: Vec<String>,
}

impl Record {
    /// Record `key`, titled `title`, a child of each of `parents`.
    fn new(key: &str, title: &str, parents: &[&str]) -> Record {
        let common = [
            ("status", Json::from("open")),
            ("priority", Json::from(2)),
            ("issue_type", Json::from("task")),
            ("created_at", Json::from(CREATED)),
            ("updated_at", Json::from(CREATED)),
            ("created_by", Json::from("seed")),
        ];
        let mut fields = Object::new();
        fields.insert("id".into(), key.into());
        fields.insert("title".into(), title.into());
        fields.extend(common.map(|(name, value)| (name.to_owned(), value)));
        Record {
            key: key.to_owned(),
            fields,
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            blocked_by: Vec::new(),
        }
    }

    /// Waiting for each of `targets`.
    fn blocked_by(mut self, targets: &[&str]) -> Record {
        self.blocked_by = targets.iter().map(|t| (*t).to_owned()).collect();
        self
    }

    /// With field `name` set to `value`: in the place of a common field of that name, else last.
    fn with(mut self, name: &str, value: impl Into<Json>) -> Record {
        self.fields.insert(name.to_owned(), value.into());
        self
    }

    /// The record as JSON, its dependencies last when it has any.
    fn json(self) -> Json {
        let key = self.key.as_str();
        let links = [
            ("parent-child", &self.parents),
            ("blocks", &self.blocked_by),
        ];
        let deps: Vec<Json> = links
            .into_iter()
            .flat_map(|(kind, targets)| {
                targets.iter().map(move |to| {
                    json!({"issue_id": key, "depends_on_id": (to.as_str()), "type": kind,
                           "created_at": CREATED, "created_by": "seed"})
                })
            })
            .collect();
        let mut fields = self.fields;
        if !deps.is_empty() {
            fields.insert("dependencies".into(), Json::Array(deps));
        }
        Json::from(fields)
    }
}

/// The tracker every test starts from (`TREE`).
fn tree() -> Vec<Json> {
    [
        Record::new("ex-e", "epic", &[]).with("issue_type", "epic"),
        Record::new("ex-n", "nested epic", &["ex-e"]).with("issue_type", "epic"),
        Record::new("ex-t", "task in the nested epic", &["ex-n"]).with("priority", 1),
        Record::new("ex-c", "closed in between", &["ex-e"])
            .with("status", "closed")
            .with("closed_at", "2026-01-02T00:00:00Z"),
        Record::new("ex-c1", "beneath the closed one", &["ex-c"])
            .with("created_at", "2026-01-03T00:00:00Z"),
        Record::new("ex-d", "deferred in between", &["ex-e"]).with("status", "deferred"),
        Record::new("ex-d1", "beneath the deferred one", &["ex-d"]).with("priority", 0),
        Record::new("ex-f", "feature, ready with its children", &["ex-e"])
            .with("issue_type", "feature")
            .with("labels", json!(["front"])),
        Record::new("ex-f1", "task in the feature", &["ex-f"]).with("priority", 3),
        Record::new(
            "ex-s",
            "reached twice, and a child of an outsider",
            &["ex-n", "ex-f", "ex-x"],
        )
        .with("created_at", "2026-01-02T00:00:00Z"),
        Record::new("ex-x", "outsider", &[]),
        Record::new("ex-g", "gate outside", &[]),
        Record::new("ex-y", "outsider held by the gate", &[]).blocked_by(&["ex-g"]),
        Record::new("ex-q", "held through its second parent", &["ex-e", "ex-y"]),
        Record::new("ex-e.99", "a child of ex-e by its key only", &[]),
        Record::new("ex-h", "a root held by the gate", &[]).blocked_by(&["ex-g"]),
        Record::new("ex-h1", "beneath the held root", &["ex-h"]),
    ]
    .into_iter()
    .map(Record::json)
    .collect()
}

/// `rows` as JSON lines in `records-N.jsonl` at the sandbox's root, N the number of rows.
fn records(sandbox: &Sandbox, rows: &[Json]) -> PathBuf {
    let path = sandbox.root().join(format!("records-{}.jsonl", rows.len()));
    let text: String = rows
        .iter()
        .map(|row| write(row, Format::ASCII).unwrap() + "\n")
        .collect();
    std::fs::write(&path, text).unwrap();
    path
}

/// `setUp`: machine a's tracker `ex`, made by importing the tree.
fn set_up() -> (Sandbox, Machine) {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let tree = records(&sandbox, &tree());
    a.bead(["init", "--prefix", "ex", "--import", tree.to_str().unwrap()]);
    (sandbox, a)
}

/// The ids of a list of issues, in its order.
fn id_list(issues: &Json) -> Vec<String> {
    let issues = issues.as_array().unwrap();
    issues
        .iter()
        .map(|issue| issue["id"].as_str().unwrap().to_owned())
        .collect()
}

/// The ids `bead ready ARGS` offers on `machine`, in its order.
fn ids(machine: &Machine, args: &[&str]) -> Vec<String> {
    id_list(&machine.bead(["ready"].iter().chain(args)).unwrap())
}

/// `ids` as a set.
fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

/// J01: `ready --epic E` matched E's direct children only, so with E -> nested epic N -> task T
/// the global frontier held T and the scoped one was empty; br's --epic is recursive.
#[test]
fn j01_the_subtree_at_any_depth_judged_on_the_whole_tracker() {
    let (_sandbox, a) = set_up();
    let before = a.export();
    let (everywhere, scoped) = (
        a.bead(["ready"]).unwrap(),
        a.bead(["ready", "ex-e"]).unwrap(),
    );
    let everywhere_ids: BTreeSet<String> = id_list(&everywhere).into_iter().collect();
    let mut wanted = set(&SUBTREE);
    wanted.extend(set(&["ex-x", "ex-g", "ex-e.99"]));
    assert_eq!(everywhere_ids, wanted);
    assert_eq!(id_list(&scoped), SUBTREE);
    let inside: Vec<Json> = everywhere
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| SUBTREE.contains(&i["id"].as_str().unwrap()))
        .cloned()
        .collect();
    // same records, same order, once each
    assert_eq!(scoped, Json::Array(inside));
    // as br's --epic
    assert_eq!(a.bead(["ready", "--epic", "ex-e"]).unwrap(), scoped);
    // the prefix may be left out, as everywhere
    assert_eq!(a.bead(["ready", "e"]).unwrap(), scoped);
    let roots: [(&str, &[&str]); 6] = [
        ("ex-n", &["ex-t", "ex-s"]),
        // a ready parent and its ready children
        ("ex-f", &["ex-s", "ex-f1"]),
        // every parent link counts
        ("ex-x", &["ex-s"]),
        // a closed root still has a subtree
        ("ex-c", &["ex-c1"]),
        // its prerequisite holds what is beneath it, and is not beneath it
        ("ex-h", &[]),
        // a leaf
        ("ex-t", &[]),
    ];
    for (root, wanted) in roots {
        assert_eq!(ids(&a, &[root]), wanted, "ready {root}");
    }
    assert_eq!(a.run(["ready", "ex-t"]).stdout.trim(), "Nothing is ready");
    // --parent stays the direct children, here and in list
    assert_eq!(ids(&a, &["--parent", "ex-e"]), ["ex-f"]);
    let children = a
        .bead(["list", "--parent", "ex-e", "--all", "--limit", "0"])
        .unwrap();
    let children: BTreeSet<String> = id_list(&children["issues"]).into_iter().collect();
    assert_eq!(children, set(&["ex-n", "ex-c", "ex-d", "ex-f", "ex-q"]));
    // filters, order and limit apply within the subtree as they do everywhere
    let filtered: [(&[&str], &[&str]); 5] = [
        (
            &["ex-e", "--type", "task"],
            &["ex-d1", "ex-t", "ex-s", "ex-c1", "ex-f1"],
        ),
        (&["ex-e", "--label", "front"], &["ex-f"]),
        (&["ex-e", "--priority", "2"], &["ex-f", "ex-s", "ex-c1"]),
        (&["ex-e", "--limit", "2"], &SUBTREE[..2]),
        (&["ex-e", "--limit", "0"], &SUBTREE),
    ];
    for (args, wanted) in filtered {
        assert_eq!(ids(&a, args), wanted, "ready {args:?}");
    }
    let human = a.run(["ready", "ex-e"]).stdout;
    let rows: Vec<&str> = human
        .lines()
        .map(|row| row.split_whitespace().nth(1).unwrap())
        .collect();
    assert_eq!(rows, SUBTREE);
    assert_eq!(a.export(), before);
}

/// J01: inside the subtree an owner, a provisional claim or a disputed status takes an issue off
/// the frontier as it does everywhere, and the walk goes on beneath a claimed issue.
#[test]
fn j01_claims_and_disputed_statuses_count_inside_as_everywhere() {
    let (sandbox, a) = set_up();
    a.bead(["claim", "ex-f"]);
    a.bead(["claim", "ex-t", "--offline"]);
    assert_eq!(ids(&a, &["ex-e"]), ["ex-d1", "ex-s", "ex-c1", "ex-f1"]);
    assert_eq!(ids(&a, &["ex-f"]), ["ex-s", "ex-f1"]);
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);
    a.bead(["update", "ex-c1", "--status", "deferred", "--offline"]);
    // the later write, so open is the value shown
    b.bead(["update", "ex-c1", "--status", "open", "--offline"]);
    for machine in [&a, &b, &a] {
        machine.bead(["sync"]);
    }
    for machine in [&a, &b] {
        let name = machine.name();
        assert_eq!(
            machine.show("ex-c1")["status"].as_str(),
            Some("open"),
            "{name}"
        );
        let shown = machine.show("ex-c1");
        let conflicts = shown["conflicts"].as_object().unwrap();
        assert!(conflicts.contains_key("status"), "{name}: {conflicts:?}");
        let inside = ids(machine, &["ex-e"]);
        assert_eq!(inside, ["ex-d1", "ex-s", "ex-f1"], "{name}");
        let everywhere = ids(machine, &[]);
        assert!(!everywhere.contains(&"ex-c1".to_owned()), "{name}");
    }
}

/// J01: the walk ends on a merged or imported parent loop, never counts the root as its own
/// descendant, and has no depth limit: 1,100 levels, everything in between closed.
#[test]
fn j01_a_loop_through_the_root_and_a_chain_deeper_than_the_recursion_limit() {
    let (sandbox, a) = set_up();
    let mut rows = vec![
        Record::new("ex-k", "in a loop", &["ex-m"]).json(),
        Record::new("ex-m", "in the same loop", &["ex-k"]).json(),
        Record::new("ex-z", "beneath the loop", &["ex-m"]).json(),
    ];
    for n in 0..1100_u32 {
        let parent = n.checked_sub(1).map(|p| format!("ex-{p}"));
        let parents: Vec<&str> = parent.iter().map(String::as_str).collect();
        let level = Record::new(&format!("ex-{n}"), &format!("level {n}"), &parents);
        let level = if [0, 1099].contains(&n) {
            level
        } else {
            level.with("status", "closed")
        };
        rows.push(level.json());
    }
    let path = records(&sandbox, &rows);
    a.bead(["import", path.to_str().unwrap()]);
    assert_eq!(ids(&a, &["ex-k"]), ["ex-m", "ex-z"]);
    assert_eq!(ids(&a, &["ex-m"]), ["ex-k", "ex-z"]);
    assert_eq!(ids(&a, &["ex-0"]), ["ex-1099"]);
}

/// J02: the scope was resolved inside the candidate loop, so on an empty tracker, or once another
/// filter had left no candidate, `ready --epic missing` answered [] and a typo passed for an
/// exhausted queue.
#[test]
fn j02_a_scope_that_names_no_issue_is_an_error_whatever_else_filters() {
    let (_sandbox, a) = set_up();
    let fresh = crate::sandbox();
    let empty = Machine::new(&fresh, "empty");
    empty.bead(["init", "--prefix", "ex"]);
    let argvs: [&[&str]; 7] = [
        &["ready", "missing"],
        &["ready", "--epic", "missing"],
        &["ready", "--parent", "missing"],
        &["list", "--parent", "missing"],
        &["ready", ""],
        &["ready", "--epic", ""],
        &["list", "--parent", ""],
    ];
    let cases: [(&Machine, &[&str]); 2] = [(&empty, &[]), (&a, &["--label", "absent"])];
    for argv in argvs {
        for (machine, filters) in cases {
            let label = format!("{argv:?} on {}", machine.name());
            let proc = machine.run(argv.iter().chain(filters).chain(&["--json"]));
            assert_eq!(
                (proc.returncode(), proc.stdout.as_str()),
                (1, ""),
                "{label}: {}",
                proc.stderr
            );
            assert_eq!(code(&proc), "ISSUE_NOT_FOUND", "{label}");
        }
    }
}

/// J03: `ready --parent A --epic B` silently ignored B. Two scopes are refused, as br refuses
/// --epic with --parent and either one given twice, and `help ready` says what each one means.
#[test]
fn j03_one_scope_at_a_time() {
    let (_sandbox, a) = set_up();
    let argvs: [&[&str]; 7] = [
        &["ready", "ex-e", "--epic", "ex-e"],
        &["ready", "ex-e", "--parent", "ex-f"],
        &["ready", "--epic", "ex-e", "--parent", "ex-f"],
        &["ready", "--parent", "ex-f", "ex-e"],
        &["ready", "--epic", "ex-e", "--epic", "ex-n"],
        &["ready", "--parent", "ex-e", "--parent", "ex-f"],
        &["list", "--parent", "ex-e", "--parent", "ex-f"],
    ];
    for argv in argvs {
        let proc = a.run(argv.iter().chain(&["--json"]));
        assert_eq!(
            (proc.returncode(), proc.stdout.as_str()),
            (2, ""),
            "{argv:?}: {}",
            proc.stderr
        );
        assert_eq!(code(&proc), "USAGE", "{argv:?}");
    }
    let help = a.run(["help", "ready"]).stdout;
    let usage = help.split_whitespace().collect::<Vec<_>>().join(" ");
    for meaning in [
        "ID's subtree",
        "--epic ID the same as ID",
        "--parent ID only ID's direct children",
    ] {
        assert!(usage.contains(meaning), "no {meaning:?} in {usage}");
    }
}
