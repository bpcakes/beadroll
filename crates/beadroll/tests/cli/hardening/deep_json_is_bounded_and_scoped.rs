//! G30: excessive JSON nesting has a bounded, scoped failure. An over-deep journal line produces
//! one integrity problem; tracked configuration and manifests produce structured errors; import
//! refuses records beyond the writing limit. A peer must keep working beside the refused import.

use std::collections::BTreeSet;

use beadroll_core::json::{Format, Object, write};
use beadroll_test_support::{
    Json, Machine, Run, Sandbox, code, json_ids, json_line, last_line, parse_json, path_text,
};

use crate::commands::fixtures::record_fields;

/// The JSON record without its closing brace, so more members can be appended.
fn head(key: &str, title: &str) -> String {
    let mut members = Object::new();
    members.insert("id".into(), Json::from(key));
    members.insert("title".into(), Json::from(title));
    if let Json::Object(common) = record_fields() {
        members.extend(*common);
    }
    let mut written = write(&Json::from(members), Format::ASCII).expect("a record");
    written.pop(); // its closing brace
    written
}

/// `depth` arrays, each inside the one before: `"[" * depth + "]" * depth`.
fn nested(depth: usize) -> String {
    format!("{}{}", "[".repeat(depth), "]".repeat(depth))
}

/// A foreign record nested beyond the reader's limit.
fn foreign() -> String {
    nested(1_000_000)
}

/// A Beads record whose unknown field nests `depth` arrays (the record itself is
/// one more level).
fn record(key: &str, depth: usize, field: &str) -> String {
    format!("{}, \"{field}\": {}}}\n", head(key, "deep"), nested(depth))
}

/// The last `count` Unicode scalar values of `text`.
fn last_chars(text: &str, count: usize) -> String {
    let skip = text.chars().count().saturating_sub(count);
    text.chars().skip(skip).collect()
}

/// Machine `g30`'s tracker (prefix `ex`) holds `ordinary work`.
fn set_up(sandbox: &Sandbox) -> Machine {
    let m = Machine::new(sandbox, "g30");
    m.bead(["init", "--prefix", "ex"]);
    m.bead(["create", "ordinary work"]);
    m
}

/// `assert_no_traceback`: `what`, which `run` ran, did not crash.
fn assert_no_crash(run: &Run, what: &str) {
    assert!(!run.crashed(), "{what} crashed: {}", last_line(&run.stderr));
}

/// `assert_works`: `what`, which `run` ran, did not crash and succeeded.
fn assert_works(run: &Run, what: &str) {
    assert_no_crash(run, what);
    let said = last_chars(&run.stderr, 400);
    assert_eq!(run.returncode(), 0, "{what}: {said}");
}

/// G30: one journal line nested deeper than json can parse (foreign segment or attempt file)
/// is reported as an unreadable line, not a RecursionError that takes down every command.
#[test]
fn line_too_deep_for_json_is_one_integrity_problem() {
    let sandbox = crate::sandbox();
    let m = set_up(&sandbox);
    let folder = m.store().join("events").join("fffffff0");
    std::fs::create_dir_all(folder.join("claims")).expect("a foreign replica's folder");
    std::fs::write(folder.join("000001.jsonl"), foreign() + "\n").expect("a deep segment");
    for command in ["list", "ready", "info"] {
        let what = format!("bead {command} with a deep segment line");
        assert_works(&m.run([command, "--json"]), &what);
    }
    let info = m.bead(["info"]).expect("bead info prints a record");
    let integrity = info.get("integrity").and_then(Json::as_array);
    let integrity = integrity.expect("info's integrity rows");
    let at_the_line = |row: &Json| {
        let origin = row.get("origin").and_then(Json::as_str);
        origin.is_some_and(|origin| origin.ends_with("000001.jsonl:1"))
    };
    assert!(integrity.iter().any(at_the_line), "{integrity:?}");
    let attempt = folder
        .join("claims")
        .join("01a1179c-0000-7000-8000-000000000001.json");
    std::fs::write(attempt, foreign() + "\n").expect("a deep attempt file");
    let listed = m.run(["list", "--json"]);
    assert_works(&listed, "bead list with a deep attempt file");
    // an unterminated deep final line is a torn tail to repair, also on sync
    let torn = m.store().join("events").join("fffffff1");
    std::fs::create_dir(&torn).expect("another foreign replica's folder");
    std::fs::write(torn.join("000001.jsonl"), foreign()).expect("a deep unterminated tail");
    let synced = m.run(["sync", "--json"]);
    assert_works(&synced, "bead sync with a deep unterminated tail");
}

/// G30: a committed .beadroll.json or a published manifest.json nested too deep for json gives
/// CONFIG / MANIFEST_MISSING (and help still works), not a traceback.
#[test]
fn deep_tracked_config_or_manifest_is_a_structured_error() {
    let sandbox = crate::sandbox();
    let m = set_up(&sandbox);
    m.bead(["sync"]);
    let tip = sandbox.origin_plumbing(&["rev-parse", "refs/beads/journal"], None);
    let deep = foreign();
    let blob = sandbox.origin_plumbing(&["hash-object", "-w", "--stdin"], Some(deep.as_bytes()));
    let listing = format!("100644 blob {blob}\tmanifest.json\n");
    let tree = sandbox.origin_plumbing(&["mktree"], Some(listing.as_bytes()));
    let commit_tree = ["commit-tree", &tree, "-p", &tip, "-m", "deep manifest"];
    let commit = sandbox.origin_plumbing(&commit_tree, None);
    sandbox.origin_plumbing(&["update-ref", "refs/beads/journal", &commit, &tip], None);
    let sync = m.run(["sync", "--json"]);
    assert_no_crash(&sync, "sync against a deep manifest");
    assert_eq!(code(&sync), "MANIFEST_MISSING");

    let config = format!("{{\"workspace\": \"ws-g30-0001\", \"x\": {deep}}}");
    std::fs::write(m.checkout().join(".beadroll.json"), config).expect("a deep config");
    let listed = m.run(["list", "--json"]);
    assert_no_crash(&listed, "list beside a deep .beadroll.json");
    assert_eq!(code(&listed), "CONFIG");
    let help = m.run(["help"]);
    assert_works(&help, "bead help beside a deep .beadroll.json");
}

/// G30: import must refuse a record nested 20,000 levels deep with VALIDATION before writing
/// any part of it.
#[test]
fn import_refuses_nesting_that_not_every_machine_can_read() {
    let sandbox = crate::sandbox();
    let m = set_up(&sandbox);
    let dependency = format!(
        "{}, \"dependencies\": [{{\"issue_id\": \"ex-depmeta\", \"depends_on_id\": \
         \"ex-elsewhere\", \"type\": \"blocks\", \"metadata\": {}}}]}}\n",
        head("ex-depmeta", "m"),
        nested(200)
    );
    let cases = [
        ("deep-20000", record("ex-deep", 20_000, "x_state")),
        ("deep-200", record("ex-deep200", 200, "x_state")),
        ("deep-dep-metadata", dependency),
    ];
    for (name, contents) in cases {
        let path = sandbox.root().join(format!("{name}.jsonl"));
        std::fs::write(&path, contents).expect("the records to import");
        let run = m.run(["import", path_text(&path), "--json"]);
        assert_no_crash(&run, &format!("import of {name}"));
        assert_ne!(
            run.returncode(),
            0,
            "import of {name} was accepted: {}",
            run.stdout
        );
        assert_eq!(code(&run), "VALIDATION", "{name}");
    }
    let listed = m.bead(["list", "--all", "--limit", "0"]);
    let listed = listed.expect("bead list prints a record");
    let issues = listed["issues"].as_array().expect("a list of issues");
    let titles: BTreeSet<&str> = issues
        .iter()
        .map(|issue| issue["title"].as_str().expect("a title"))
        .collect();
    assert_eq!(titles, BTreeSet::from(["ordinary work"]));
    let ordinary = sandbox.root().join("nested-50.jsonl");
    let nested_50 = record("ex-nested", 50, "custom");
    std::fs::write(&ordinary, nested_50).expect("the record to import");
    let imported = m.bead(["import", path_text(&ordinary)]);
    let imported = imported.expect("bead import prints a record");
    assert_eq!(imported["imported"].as_i64(), Some(1));
    let export = m.export();
    let row = export.split('\n').find(|row| row.contains("\"ex-nested\""));
    let exported = parse_json(row.expect("the record in the export")).expect("a record");
    let custom = exported.get("custom").expect("its custom field");
    let written = json_line(custom);
    assert_eq!(written, nested(50));
    // the bound itself: a create holds its record two levels down, so 100 levels allow a record 98
    // deep (its object, then 97 arrays) and refuse one a level deeper
    for (depth, accepted) in [(97, true), (98, false)] {
        let path = sandbox.root().join(format!("bound-{depth}.jsonl"));
        let key = format!("ex-bound{depth}");
        std::fs::write(&path, record(&key, depth, "custom")).expect("the record to import");
        let run = m.run(["import", path_text(&path), "--json"]);
        assert_no_crash(&run, &format!("import {depth} deep"));
        if accepted {
            assert_eq!(run.returncode(), 0, "{depth}: {}", run.stderr);
        } else {
            assert_eq!(code(&run), "VALIDATION", "{depth}: {}", run.stderr);
            assert!(run.stderr.contains("more than 98 levels"), "{}", run.stderr);
        }
        let listed = m.bead(["list", "--all", "--id", &key, "--limit", "0"]);
        let total = listed.expect("bead list prints a record")["total"].as_i64();
        assert_eq!(total, Some(i64::from(accepted)), "{depth}");
    }
}

/// The ids of the issues a `bead list --json` printed.
fn ids(listed: &Json) -> BTreeSet<String> {
    json_ids(&listed["issues"]).into_iter().collect()
}

/// G30: after one machine's 12,000-level import is refused, its peer keeps working and sees
/// the same state.
#[test]
fn peer_keeps_working_after_a_deep_import_elsewhere() {
    let sandbox = crate::sandbox();
    let a = set_up(&sandbox);
    a.bead(["sync"]);
    let b = Machine::new(&sandbox, "g30b");
    assert_works(&b.run(["sync", "--json"]), "the peer's first sync");
    let path = sandbox.root().join("deep.jsonl");
    std::fs::write(&path, record("ex-deep", 12_000, "x_state")).expect("the record");
    let imported = a.run(["import", path_text(&path), "--json"]);
    assert_no_crash(&imported, "the deep import");
    let accepted = imported.returncode() == 0;
    assert!(
        !accepted,
        "the deep record was imported: {}",
        imported.stdout
    );
    assert_eq!(code(&imported), "VALIDATION", "the deep import");
    a.bead(["create", "made after the import"]);
    a.bead(["sync"]);
    let synced = b.run(["sync", "--json"]);
    assert_works(&synced, "the peer's sync after the import");
    let listed = b.run(["list", "--all", "--limit", "0", "--json"]);
    assert_works(&listed, "the peer's list");
    let seen = parse_json(&listed.stdout).expect("bead list prints a record");
    let first = a.bead(["list", "--all", "--limit", "0"]);
    assert_eq!(ids(&seen), ids(&first.expect("bead list prints a record")));
    assert_eq!(ids(&seen).len(), 2, "{seen:?}");
}
