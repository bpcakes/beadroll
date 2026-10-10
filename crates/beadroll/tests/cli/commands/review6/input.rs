//! F07, F08, F09 and a verified property of F09's fix: what bead reads. Text that is not UTF-8, on
//! the command line or in a file, is a structured error; an import checks its dependencies before
//! it writes; a dependency taken back before its target exists stays taken back; and a mutual
//! dependency imported in two batches is the one a single import makes.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::json::lone_surrogate_escapes;
use beadroll_test_support::{Json, Machine, code, json, parse_json};

use super::{id, jsonl, machine, record};

/// F07: non-UTF-8 titles and description files return a JSON error and store nothing.
#[test]
fn f07_non_utf8_input_is_a_structured_error() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "f07");
    // Python passes the title "bad\udcfftitle" through `os.fsencode`: the lone surrogate is the
    // byte 0xFF.
    let title = OsStr::from_bytes(b"bad\xfftitle");
    let created = machine
        .cmd([OsStr::new("create"), title, OsStr::new("--json")])
        .run();
    assert_ne!(created.returncode(), 0, "{}", created.stdout);
    assert!(!created.crashed(), "{}", created.stderr);
    assert_eq!(code(&created), "VALIDATION", "{}", created.stderr);
    // No title holds U+DCFF. Python's `json.loads` would keep that lone surrogate in a title, where
    // Rust's reads it as U+FFFD: so look for its escape in the listing as printed.
    let listed = machine.run(["list", "--all", "--limit", "0", "--json"]);
    assert_eq!(listed.returncode(), 0, "{}", listed.stderr);
    let titles: Vec<_> = parse_json(&listed.stdout).unwrap()["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["title"].clone())
        .collect();
    let escaped = lone_surrogate_escapes(&listed.stdout);
    assert!(
        escaped.iter().all(|(_, unit)| *unit != 0xdcff),
        "a title holds U+DCFF: {titles:?} in {}",
        listed.stdout
    );

    let raw = sandbox.root().join("raw.txt");
    std::fs::write(&raw, b"caf\xff").unwrap();
    let key = id(machine.bead(["create", "file target"]));
    let updated = machine.run([
        "update",
        &key,
        "--description-file",
        raw.to_str().unwrap(),
        "--json",
    ]);
    assert_ne!(updated.returncode(), 0, "{}", updated.stdout);
    assert!(!updated.crashed(), "{}", updated.stderr);
    assert_eq!(code(&updated), "VALIDATION", "{}", updated.stderr);
    let shown = machine.show(&key);
    assert!(shown.get("description").is_none(), "{shown:?}");
}

/// F08: a bad dependency fails the import; it is not dropped and it is not a traceback.
#[test]
fn f08_import_probes_dependencies_before_writing() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "f08");
    let typed = sandbox.root().join("typed.jsonl");
    let dependency = json!({"issue_id": "ex-a", "depends_on_id": "ex-b", "type": 1});
    let records = [
        record("ex-a", "A", json!({"dependencies": [dependency]})),
        record("ex-b", "B", json!({})),
    ];
    std::fs::write(&typed, jsonl(&records)).unwrap();
    let imported = machine.run(["import", typed.to_str().unwrap(), "--json"]);
    assert_ne!(imported.returncode(), 0, "{}", imported.stdout);
    assert!(!imported.crashed(), "{}", imported.stderr);
    assert_eq!(code(&imported), "VALIDATION", "{}", imported.stderr);
    assert_eq!(machine.bead(["info"]).unwrap()["integrity"], json!([]));

    let bare = sandbox.root().join("bare.jsonl");
    let records = [record("ex-c", "C", json!({"dependencies": ["ex-b"]}))];
    std::fs::write(&bare, jsonl(&records)).unwrap();
    let imported = machine.run(["import", bare.to_str().unwrap(), "--json"]);
    assert_ne!(imported.returncode(), 0, "{}", imported.stdout);
    assert!(!imported.crashed(), "{}", imported.stderr);
    let listed = machine.bead(["list", "--all", "--id", "ex-c", "--limit", "0"]);
    assert_eq!(listed.unwrap()["total"].as_i64(), Some(0));
}

/// The dependencies `bead show KEY` lists, as sorted (id, type) pairs.
fn edges(machine: &Machine, key: &str) -> Vec<(String, String)> {
    let shown = machine.show(key);
    let text = |value: &Json| value.as_str().unwrap().to_owned();
    let mut edges: Vec<_> = shown["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| (text(&item["id"]), text(&item["dependency_type"])))
        .collect();
    edges.sort();
    edges
}

/// A record's `dependencies`: `key` is blocked by `blocker`.
fn blocked_by(key: &str, blocker: &str) -> Json {
    let dependency = json!({"issue_id": key, "depends_on_id": blocker, "type": "blocks"});
    json!({"dependencies": [dependency]})
}

/// Verified (no finding): a mutual dependency imported in two batches matches one import.
#[test]
fn incremental_mutual_import_matches_one_shot() {
    let sandbox = crate::sandbox();
    let root = sandbox.root();
    let first = record("ex-a", "A", blocked_by("ex-a", "ex-b"));
    let second = record("ex-b", "B", blocked_by("ex-b", "ex-a"));
    std::fs::write(root.join("a.jsonl"), jsonl(std::slice::from_ref(&first))).unwrap();
    std::fs::write(root.join("b.jsonl"), jsonl(std::slice::from_ref(&second))).unwrap();
    std::fs::write(root.join("both.jsonl"), jsonl(&[first, second])).unwrap();
    let path = |name: &str| root.join(name).to_str().unwrap().to_owned();
    let split = machine(&sandbox, "mutual-split");
    split.bead(["import", &path("a.jsonl")]);
    split.bead(["import", &path("b.jsonl")]);
    let one = machine(&sandbox, "mutual-one");
    one.bead(["import", &path("both.jsonl")]);

    assert_eq!(edges(&split, "ex-a"), edges(&one, "ex-a"));
    assert_eq!(edges(&split, "ex-b"), edges(&one, "ex-b"));
    assert_eq!(split.ready(), one.ready());
    assert_eq!(split.ready(), BTreeSet::new());
}

/// F09: a dependency removed before its target exists does not reappear, and does not cycle (the
/// command-line subtests; the replay one is beadroll-core's `commands::review6`).
#[test]
fn f09_dangling_dependency_can_be_retracted_before_its_target_exists() {
    let sandbox = crate::sandbox();
    let root = sandbox.root();
    let dangling = record(
        "ex-a",
        "A",
        json!({"dependencies": [{"issue_id": "ex-a", "depends_on_id": "ex-b", "type": "blocks"}]}),
    );
    let cleared = record("ex-a", "A", json!({}));
    let target = record(
        "ex-b",
        "B",
        json!({"dependencies": [{"issue_id": "ex-b", "depends_on_id": "ex-a", "type": "blocks"}]}),
    );
    for (name, records) in [("a", dangling), ("cleared", cleared), ("b", target)] {
        let text = jsonl(std::slice::from_ref(&records));
        std::fs::write(root.join(format!("{name}.jsonl")), text).unwrap();
    }
    let file = |name: &str| root.join(format!("{name}.jsonl")).display().to_string();

    // dep remove
    let retracted = machine(&sandbox, "f09");
    retracted.bead(["import", &file("a")]);
    let removed = retracted.run(["dep", "remove", "ex-a", "ex-b", "--json"]);
    assert_eq!(removed.returncode(), 0, "dep remove: {}", removed.stderr);
    retracted.bead(["import", &file("b")]);
    assert_eq!(
        retracted.show("ex-a").get("dependencies"),
        Some(&Json::Array(Vec::new())),
        "dep remove"
    );
    let ready = retracted.ready();
    assert!(ready.contains("ex-a"), "dep remove: {ready:?}");
    assert!(!ready.contains("ex-b"), "dep remove: {ready:?}");

    // reimport
    let reimported = machine(&sandbox, "f09-reimport");
    for name in ["a", "cleared", "b"] {
        reimported.bead(["import", &file(name)]);
    }
    let shown = reimported.show("ex-a");
    let deps: Vec<(String, String)> = shown["dependencies"]
        .as_array()
        .unwrap_or_default()
        .iter()
        .map(|d| (str_of(&d["id"]), str_of(&d["dependency_type"])))
        .collect();
    assert_eq!(deps, Vec::<(String, String)>::new(), "reimport");
    assert!(reimported.ready().contains("ex-a"), "reimport");
    let cycles = reimported.bead(["dep", "cycles"]).unwrap();
    assert_eq!(cycles["count"].as_i64(), Some(0), "reimport");
}

/// The text of a JSON string.
fn str_of(value: &Json) -> String {
    value.as_str().expect("a string").to_owned()
}
