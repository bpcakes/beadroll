//! F07–F09, E04, E08, Q06, Q10, R13, U09, U11 and a verified property of F09's fix: what bead
//! reads. Text that is not UTF-8, on the command line or in a file, is a structured error; an
//! import checks its dependencies before it writes, fails whole on a record it cannot take (one
//! without a creation time, or with a lone surrogate anywhere), takes null where a field may be,
//! renumbers no comment already exported, and keeps a reserved field it imports from breaking the
//! export; a dependency taken back before its target exists stays taken back; and an import split
//! in two, or a mutual dependency imported in two batches, is the one a single import makes.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;

use beadroll_core::json::Object;
use beadroll_test_support::{
    Json, Machine, code, json, json_line, json_lines, json_member, parse_json, printed_id,
};

use super::fixtures::{STAND_IN, machine, published_issue, record, record_text, strict_json};

/// F07: non-UTF-8 titles and description files return a JSON error and store nothing.
#[test]
fn non_utf8_input_is_a_structured_error() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "f07");
    // The invalid byte 0xFF appears within the supplied title.
    let title = OsStr::from_bytes(b"bad\xfftitle");
    let created = machine
        .cmd([OsStr::new("create"), title, OsStr::new("--json")])
        .run();
    assert_ne!(created.returncode(), 0, "{}", created.stdout);
    assert!(!created.crashed(), "{}", created.stderr);
    assert_eq!(code(&created), "VALIDATION", "{}", created.stderr);
    // No issue was created: the title is nowhere, as itself, as U+FFFD or as an escape.
    let listed = machine.run(["list", "--all", "--limit", "0", "--json"]);
    assert_eq!(listed.returncode(), 0, "{}", listed.stderr);
    let titles: Vec<_> = parse_json(&listed.stdout).unwrap()["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["title"].clone())
        .collect();
    assert_eq!(titles, Vec::<Json>::new());
    assert!(
        !listed.stdout.contains("bad") && !listed.stdout.contains("\\ud"),
        "{}",
        listed.stdout
    );

    let raw = sandbox.root().join("raw.txt");
    std::fs::write(&raw, b"caf\xff").unwrap();
    let key = printed_id(machine.bead(["create", "file target"]));
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

/// An import keeps a cycle its records form between them, as the export has it, where `dep add`
/// would refuse the edge that closes it: both issues come in, and `dep cycles` reports the loop
/// (the maintainer's decision of 2026-11-18, `docs/REVIEWING.md` §2).
#[test]
fn import_keeps_a_cycle_its_records_form_and_dep_cycles_reports_it() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "cycled");
    let on = |from: &str, to: &str| json!({"dependencies": [{"issue_id": from, "depends_on_id": to, "type": "blocks"}]});
    let records = [
        record("ex-a", "A", on("ex-a", "ex-b")),
        record("ex-b", "B", on("ex-b", "ex-a")),
    ];
    let looped = sandbox.root().join("looped.jsonl");
    std::fs::write(&looped, json_lines(&records)).unwrap();
    let imported = machine.run(["import", looped.to_str().unwrap(), "--json"]);
    assert_eq!(imported.returncode(), 0, "{}", imported.stderr);
    let cycles = machine.bead(["dep", "cycles"]).expect("the cycles");
    assert_eq!(cycles["count"].as_i64(), Some(1), "{cycles:?}");
    // the same edge given by hand is refused
    let by_hand = machine.run(["dep", "add", "ex-b", "ex-a", "--json"]);
    assert_eq!(code(&by_hand), "CYCLE", "{}", by_hand.stderr);
}

/// F08: a bad dependency fails the import; it is not dropped and it is not a traceback.
#[test]
fn import_probes_dependencies_before_writing() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "f08");
    let typed = sandbox.root().join("typed.jsonl");
    let dependency = json!({"issue_id": "ex-a", "depends_on_id": "ex-b", "type": 1});
    let records = [
        record("ex-a", "A", json!({"dependencies": [dependency]})),
        record("ex-b", "B", json!({})),
    ];
    std::fs::write(&typed, json_lines(&records)).unwrap();
    let imported = machine.run(["import", typed.to_str().unwrap(), "--json"]);
    assert_ne!(imported.returncode(), 0, "{}", imported.stdout);
    assert!(!imported.crashed(), "{}", imported.stderr);
    assert_eq!(code(&imported), "VALIDATION", "{}", imported.stderr);
    assert_eq!(machine.bead(["info"]).unwrap()["integrity"], json!([]));
    // probed before writing: neither record's create was written
    let none = machine
        .bead(["list", "--all", "--limit", "0"])
        .expect("a list");
    assert_eq!(none["total"], json!(0), "{none:?}");

    let bare = sandbox.root().join("bare.jsonl");
    let records = [record("ex-c", "C", json!({"dependencies": ["ex-b"]}))];
    std::fs::write(&bare, json_lines(&records)).unwrap();
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
    std::fs::write(
        root.join("a.jsonl"),
        json_lines(std::slice::from_ref(&first)),
    )
    .unwrap();
    std::fs::write(
        root.join("b.jsonl"),
        json_lines(std::slice::from_ref(&second)),
    )
    .unwrap();
    std::fs::write(root.join("both.jsonl"), json_lines(&[first, second])).unwrap();
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
/// command-line subtests; the replay one is beadroll-core's `commands::dangling_dependencies`).
#[test]
fn dangling_dependency_can_be_retracted_before_its_target_exists() {
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
        let text = json_lines(std::slice::from_ref(&records));
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

/// E04: an import count and exit zero must mean the issue was imported.
#[test]
fn invalid_import_fails_instead_of_counting_an_inert_event() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e04-import");
    let source = sandbox.root().join("invalid.jsonl");
    let invalid = record_text(
        json!({"id": "ex-invalid", "title": "discarded"}),
        // a lower-case custom status imports
        json!({"status": "NOT A STATUS"}),
    );
    std::fs::write(&source, invalid + "\n").unwrap();
    let imported = machine.run(["import", &source.display().to_string(), "--json"]);
    assert_ne!(imported.returncode(), 0, "{}", imported.stdout);
    let error = strict_json(&imported.stderr)["error"].clone();
    assert_eq!(error["code"].as_str(), Some("VALIDATION"), "{error:?}");
    assert_eq!(machine.bead(["info"]).unwrap()["integrity"], json!([]));
}

/// E08: values accepted by import validation must be safe in every query path.
#[test]
fn nullable_import_values_do_not_crash_render_or_export() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e08-nullable");
    let comments = sandbox.root().join("nullable-comment.jsonl");
    let commented = record_text(
        json!({"id": "ex-null", "title": "nullable comment"}),
        json!({"comments": [{"id": 1, "author": null, "text": "hello", "created_at": null}]}),
    );
    std::fs::write(&comments, commented + "\n").unwrap();
    let imported = machine.run(["import", &comments.display().to_string(), "--json"]);
    assert_eq!(imported.returncode(), 0, "{}", imported.stderr);
    for args in [["show", "ex-null"], ["comments", "ex-null"]] {
        let result = machine.run(args);
        assert_eq!(result.returncode(), 0, "{args:?}: {}", result.stderr);
    }

    let dependencies = sandbox.root().join("nullable-dependencies.jsonl");
    let depending = record_text(
        json!({"id": "ex-deps", "title": "nullable dependency"}),
        json!({"dependencies": [
            {"issue_id": "ex-deps", "depends_on_id": "missing", "type": null},
            {"issue_id": "ex-deps", "depends_on_id": "missing", "type": "blocks"}
        ]}),
    );
    std::fs::write(&dependencies, depending + "\n").unwrap();
    let imported = machine.run(["import", &dependencies.display().to_string(), "--json"]);
    assert_eq!(imported.returncode(), 0, "{}", imported.stderr);
    let exported = machine.run(["export"]);
    assert_eq!(exported.returncode(), 0, "{}", exported.stderr);
}

/// U09 (docs/FINDINGS.md): a record whose created_at is null imported, and then `list` failed
/// comparing times; the import fails whole instead, and the tracker keeps listing.
#[test]
fn a_record_without_a_creation_time_is_not_imported() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "u09-created");
    machine.bead(["create", "listed first"]);
    let source = sandbox.root().join("no-creation-time.jsonl");
    let untimed = record_text(
        json!({"id": "ex-untimed", "title": "no time"}),
        json!({"created_at": null}),
    );
    std::fs::write(&source, untimed + "\n").unwrap();
    let imported = machine.run(["import", &source.display().to_string(), "--json"]);
    assert_eq!(imported.returncode(), 1, "{}", imported.stdout);
    assert_eq!(
        strict_json(&imported.stderr)["error"],
        json!({
            "code": "VALIDATION",
            "message": "record 'ex-untimed' cannot be imported: field 'created_at' cannot be cleared"
        })
    );
    let listed = machine.bead(["list"]).unwrap();
    let issues = listed["issues"].as_array().expect("issues");
    let titles: Vec<Option<&str>> = issues.iter().map(|i| i["title"].as_str()).collect();
    assert_eq!(titles, [Some("listed first")]);
}

/// U11 (docs/FINDINGS.md): an escaped lone surrogate failed an import only where it reached an
/// event ("text is not valid UTF-8" when the create was written), and passed where the import
/// drops it (a known record, source_repo_path); every record is refused for it, as a raw one
/// already was: since roll-lqjj.53 the record is not valid JSONL at all, named by its line.
#[test]
fn an_escaped_lone_surrogate_fails_the_import_wherever_it_is() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "u11-surrogate");
    let good = record_text(json!({"id": "ex-fine", "title": "fine"}), json!({}));
    // A Rust string cannot hold a lone surrogate. Write STAND_IN instead, then replace its JSON
    // escape with the surrogate escape.
    let escape = |unit: u32| format!(r"\u{unit:04x}");
    let lone = |line: String, surrogate: u32| {
        let stand_in = escape(u32::from(STAND_IN));
        assert!(line.contains(&stand_in), "{line}");
        line.replace(&stand_in, &escape(surrogate))
    };
    let title = record_text(
        json!({"id": "ex-1"}),
        json!({"title": (format!("a{STAND_IN}"))}),
    );
    let dropped = record_text(
        json!({"id": "ex-2", "title": "b"}),
        json!({"source_repo_path": (STAND_IN.to_string())}),
    );
    let cases = [
        ("title", lone(title, 0xD800)),
        ("dropped", lone(dropped, 0xDC00)),
    ];
    for (name, bad) in cases {
        let source = sandbox.root().join(format!("{name}.jsonl"));
        // escaped: ASCII text
        std::fs::write(&source, format!("{good}\n{bad}\n")).unwrap();
        let imported = machine.run(["import", &source.display().to_string(), "--json"]);
        assert_eq!(imported.returncode(), 1, "{name}: {}", imported.stdout);
        let error = strict_json(&imported.stderr)["error"].clone();
        assert_eq!(error["code"], json!("VALIDATION"), "{name}: {error:?}");
        let message = error["message"].as_str().unwrap_or_default();
        let expected = format!("{}:2 is not valid JSONL: ", source.display());
        assert!(message.starts_with(&expected), "{name}: {message}");
    }
    assert_eq!(machine.bead(["list"]).unwrap()["issues"], json!([]));
}

/// An issue record as Q06 writes it: `id` and `title`, then the fields both of its records share.
fn issue(id: &str, title: &str) -> Object {
    let record = json!({
        "id": id, "title": title, "status": "open", "priority": 2, "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed"
    });
    record.as_object().unwrap().clone()
}

/// Q06: split and one-shot imports must have the same dependencies and readiness.
#[test]
fn incremental_import_resolves_late_dependency_target() {
    let sandbox = crate::sandbox();
    let dep = json!({
        "issue_id": "ex-a", "depends_on_id": "ex-b", "type": "blocks",
        "created_at": "2026-01-01T00:00:00Z", "created_by": "seed",
        "metadata": "{}", "thread_id": ""
    });
    let mut issue_a = issue("ex-a", "A");
    issue_a.insert("dependencies".into(), json!([dep]));
    let (issue_a, issue_b) = (Json::from(issue_a), Json::from(issue("ex-b", "B")));
    let root = sandbox.root();
    let (a_path, b_path, all_path) = (
        root.join("q06-a.jsonl"),
        root.join("q06-b.jsonl"),
        root.join("q06-all.jsonl"),
    );
    std::fs::write(&a_path, json_line(&issue_a) + "\n").unwrap();
    std::fs::write(&b_path, json_line(&issue_b) + "\n").unwrap();
    let both = json_line(&issue_a) + "\n" + &json_line(&issue_b) + "\n";
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
        json_member(&split_a, "dependencies"),
        json_member(&one_a, "dependencies")
    );
    assert_eq!(
        json_member(&split_a, "blocked"),
        json_member(&one_a, "blocked")
    );
    assert_eq!(split.ready(), one.ready());
}

/// Q10: an imported unknown field must not poison internal dependency bookkeeping.
#[test]
fn reserved_dangling_extra_cannot_crash_export() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "q10-import");
    a.bead(["init", "--prefix", "ex"]);
    let record = json!({
        "id": "ex-one", "title": "one", "status": "open", "priority": 2,
        "issue_type": "task", "_dangling_dependencies": "user supplied"
    });
    let path = sandbox.root().join("q10.jsonl");
    std::fs::write(&path, json_line(&record) + "\n").unwrap();
    a.bead(["import", path.to_str().unwrap()]);
    let exported = a.run(["export"]);
    assert_eq!(exported.returncode(), 0, "{}", exported.stderr);
    let exported = parse_json(&exported.stdout).unwrap();
    assert_eq!(exported["_dangling_dependencies"], json!("user supplied"));
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

/// R13: importing a legacy Beads record whose comment has id 41 does not renumber the comment this
/// machine had already exported.
#[test]
fn importing_legacy_comment_does_not_renumber_existing_local_comment() {
    let sandbox = crate::sandbox();
    let (a, key) = published_issue(&sandbox);
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
    let line = json_line(&legacy) + "\n";
    std::fs::write(&src, line).expect("the legacy file");
    a.bead(["import", src.to_str().expect("a UTF-8 path")]);
    let after = first_comment_id(&a.export(), &key);
    assert_eq!(
        after, before,
        "single-machine import renumbered an already exported comment"
    );
}

/// A record to import with a title that holds nothing, or a dependency that names another issue as
/// its own, is refused whole, naming it, and nothing is imported: an issue no reader can name, or a
/// dependency moved onto another record without a word, is never made.
#[test]
fn import_refuses_a_blank_title_and_a_dependency_of_another_record() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "refusals");
    let dependency = json!({"issue_id": "ex-a", "depends_on_id": "ex-x", "type": "blocks"});
    let cases = [
        (
            "blank",
            vec![record("ex-a", "   ", json!({}))],
            "has no title",
        ),
        ("empty", vec![record("ex-a", "", json!({}))], "has no title"),
        (
            "elsewhere",
            vec![
                record("ex-a", "a", json!({})),
                record("ex-b", "b", json!({"dependencies": [dependency]})),
            ],
            "lists a dependency of 'ex-a'",
        ),
    ];
    for (name, records, said) in cases {
        let file = sandbox.root().join(format!("{name}.jsonl"));
        std::fs::write(&file, json_lines(&records)).unwrap();
        let imported = machine.run(["import", file.to_str().unwrap(), "--json"]);
        assert_eq!(code(&imported), "VALIDATION", "{name}: {}", imported.stderr);
        assert!(
            imported.stderr.contains(said),
            "{name}: {}",
            imported.stderr
        );
    }
    let listed = machine
        .bead(["list", "--all", "--limit", "0"])
        .expect("a list");
    assert_eq!(listed["total"], json!(0), "{listed:?}");
}

/// `init --import` without `--prefix` starts the tracker under the prefix its records share, so
/// that what is made after joins them; records under several prefixes are `VALIDATION` asking for
/// `--prefix`, and `--prefix` given wins.
#[test]
fn init_import_takes_the_prefix_its_records_share() {
    // each its own sandbox: a second `init` beside a published tracker joins it, prefix and all
    let start = |name: &str, records: &[Json], given: &[&str]| {
        let sandbox = crate::sandbox();
        let file = sandbox.root().join(format!("{name}.jsonl"));
        std::fs::write(&file, json_lines(records)).unwrap();
        let machine = beadroll_test_support::Machine::new(&sandbox, name);
        let mut args = vec!["init"];
        args.extend_from_slice(given);
        args.extend(["--import", file.to_str().unwrap(), "--json"]);
        let run = machine.run(args);
        (sandbox, machine, run)
    };
    let shared = [
        record("bd-1", "one", json!({})),
        record("bd-2", "two", json!({})),
    ];
    let (_kept, a, run) = start("a", &shared, &[]);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    assert_eq!(a.bead(["info"]).unwrap()["prefix"], json!("bd"));
    let made = a.bead(["create", "after"]).unwrap();
    assert!(made["id"].as_str().unwrap().starts_with("bd-"), "{made:?}");
    let mixed = [
        record("bd-1", "one", json!({})),
        record("xy-2", "two", json!({})),
    ];
    let (_kept, _b, refused) = start("b", &mixed, &[]);
    assert_eq!(code(&refused), "VALIDATION", "{}", refused.stderr);
    assert!(refused.stderr.contains("--prefix"), "{}", refused.stderr);
    let (_kept, c, run) = start("c", &shared, &["--prefix", "own"]);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    assert_eq!(c.bead(["info"]).unwrap()["prefix"], json!("own"));
}

/// What the import refuses whatever shape its records take: a Beads memory (`_type: memory`), with
/// an id and a title too, which was imported as a task and exported back with its `_type`; and a
/// record keyed under no prefix (`-1`), which started a tracker whose prefix was empty.
#[test]
fn memories_and_records_without_a_prefix_are_refused() {
    let sandbox = crate::sandbox();
    let mut memory = record("ex-9", "a memory", json!({}));
    if let Json::Object(members) = &mut memory {
        members.insert("_type".into(), json!("memory"));
    }
    let cases = [
        ("memory", memory, "memory"),
        ("unprefixed", record("-1", "x", json!({})), "no prefix"),
    ];
    for (name, record, said) in cases {
        let file = sandbox.root().join(format!("{name}.jsonl"));
        std::fs::write(&file, json_lines(&[record])).unwrap();
        let machine = beadroll_test_support::Machine::new(&sandbox, name);
        let run = machine.run(["init", "--import", file.to_str().unwrap(), "--json"]);
        assert_eq!(code(&run), "VALIDATION", "{name}: {}", run.stderr);
        assert!(run.stderr.contains(said), "{name}: {}", run.stderr);
    }
}

/// A re-import that only takes back a dependency, its record imported again without it, says so,
/// `taken_back` in `--json` too: it answered `Imported 0 issues` while it wrote a `dep_remove`.
#[test]
fn a_reimport_that_takes_back_a_dependency_says_so() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "retract");
    let dependency = json!({"issue_id": "ex-7", "depends_on_id": "ex-99", "type": "blocks"});
    let with = sandbox.root().join("with.jsonl");
    std::fs::write(
        &with,
        json_lines(&[record(
            "ex-7",
            "seven",
            json!({"dependencies": [dependency]}),
        )]),
    )
    .unwrap();
    let without = sandbox.root().join("without.jsonl");
    std::fs::write(&without, json_lines(&[record("ex-7", "seven", json!({}))])).unwrap();
    machine.bead(["import", with.to_str().unwrap()]);
    let again = machine.bead(["import", without.to_str().unwrap()]).unwrap();
    assert_eq!(again["imported"], json!(0), "{again:?}");
    assert_eq!(again["taken_back"], json!(1), "{again:?}");
    let human = machine.run(["import", without.to_str().unwrap()]);
    assert_eq!(
        human.stdout.trim(),
        "Imported 0 issues",
        "nothing left to take back: {}",
        human.stdout
    );
}

/// `init --import --dry-run` reads the prefix as `init --import` would: records under several
/// prefixes are a problem it lists, where it said the import would go through, and `--prefix`,
/// which settles it, is taken.
#[test]
fn an_init_dry_run_reads_the_prefix_as_the_init_would() {
    let sandbox = crate::sandbox();
    let mixed = sandbox.root().join("mixed.jsonl");
    std::fs::write(
        &mixed,
        json_lines(&[
            record("ap-1", "a", json!({})),
            record("zz-1", "b", json!({})),
        ]),
    )
    .unwrap();
    let machine = beadroll_test_support::Machine::new(&sandbox, "plan");
    let planned = machine
        .bead(["init", "--import", mixed.to_str().unwrap(), "--dry-run"])
        .unwrap();
    assert_eq!(planned["importable"], json!(false), "{planned:?}");
    assert!(
        planned["problems"][0]
            .as_str()
            .unwrap()
            .contains("--prefix"),
        "{planned:?}"
    );
    let given = [
        "init",
        "--import",
        mixed.to_str().unwrap(),
        "--dry-run",
        "--prefix",
        "pp",
    ];
    let planned = machine.bead(given).unwrap();
    assert_eq!(planned["importable"], json!(true), "{planned:?}");
}
