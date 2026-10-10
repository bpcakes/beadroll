//! `CompatAuditCLI`, from the first independent audit: global options in any position, stdin and
//! multi-id commands, the JSON error envelopes, workspace configuration, what an import keeps, and
//! the metadata events carry.

use std::path::Path;

use beadroll_core::json::{Format, write};
use beadroll_core::text::is_space;
use beadroll_test_support::{Json, Machine, Sandbox, code, json, parse_json};

/// `setUp`: a sandbox and machine `compat`, whose tracker (prefix `ex`) holds issues X and Y.
fn set_up() -> (Sandbox, Machine, String, String) {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "compat");
    a.bead(["init", "--prefix", "ex"]);
    let x = id(&a.bead(["create", "Issue X"]).unwrap());
    let y = id(&a.bead(["create", "Issue Y"]).unwrap());
    (sandbox, a, x, y)
}

fn id(value: &Json) -> String {
    value["id"].as_str().unwrap().to_owned()
}

/// The ids of a list of issues, in order.
fn ids(issues: &Json) -> Vec<String> {
    issues.as_array().unwrap().iter().map(id).collect()
}

/// `value` as `json.dumps(value)` writes it.
fn dumps(value: &Json) -> String {
    write(value, Format::ASCII).unwrap()
}

/// The record `key` of a Beads export: the first line that names it.
fn record(export: &str, key: &str) -> Json {
    export
        .lines()
        .map(|line| parse_json(line).unwrap())
        .find(|r| r["id"].as_str() == Some(key))
        .unwrap_or_else(|| panic!("no {key} in the export: {export}"))
}

#[test]
fn global_json_actor_positions() {
    let (_sandbox, a, _, _) = set_up();
    for args in [
        &["--json", "--actor", "reviewer", "info"][..],
        &["info", "--json", "--actor", "reviewer"],
        &["--actor=reviewer", "info", "--json"],
        &["info", "--actor=reviewer", "--json"],
    ] {
        let result = a.run(args);
        assert_eq!(result.returncode(), 0, "{args:?}: {}", result.stderr);
        let info = parse_json(&result.stdout).unwrap();
        assert_eq!(info["actor"]["name"].as_str(), Some("reviewer"), "{args:?}");
    }
}

#[test]
fn description_stdin_multi_id_and_filters() {
    let (_sandbox, a, x, y) = set_up();
    let p = a
        .cmd(["update", &x, &y, "--description-file", "-", "--json"])
        .stdin("multiline\nbody\n")
        .run();
    assert_eq!(p.returncode(), 0, "{}", p.stderr);
    let updated = parse_json(&p.stdout).unwrap();
    let descriptions: Vec<Option<&str>> = updated
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["description"].as_str())
        .collect();
    assert_eq!(descriptions, [Some("multiline\nbody\n"); 2]);
    a.bead([
        "update",
        &x,
        "--add-label",
        "red,green",
        "--priority",
        "0",
        "--assignee",
        "reviewer",
        "--type",
        "bug",
    ]);
    assert_eq!(a.bead(["list", "--limit", "0"]).unwrap()["total"], json!(2));
    let filtered = a
        .bead([
            "list",
            "--label",
            "red",
            "--label",
            "green",
            "--priority",
            "0",
            "--assignee",
            "reviewer",
            "--type",
            "bug",
            "--title-contains",
            "x",
        ])
        .unwrap();
    assert_eq!(ids(&filtered["issues"]), std::slice::from_ref(&x));
    assert_eq!(ids(&a.bead(["show", &x, &y]).unwrap()), [x, y]);
}

#[test]
fn multi_id_update_rolls_back_on_missing_issue() {
    let (_sandbox, a, x, _) = set_up();
    let before = a.export();
    let p = a.run(["update", &x, "missing", "--title", "changed", "--json"]);
    assert_eq!(p.returncode(), 1);
    assert_eq!(code(&p), "ISSUE_NOT_FOUND");
    assert_eq!(a.export(), before);
}

#[test]
fn domain_error_json_envelope() {
    let (_sandbox, a, _, _) = set_up();
    let p = a.run(["show", "missing", "--json"]);
    assert_eq!(p.returncode(), 1);
    assert_eq!(p.stdout, "");
    assert_eq!(code(&p), "ISSUE_NOT_FOUND");
}

#[test]
fn parser_error_json_envelope() {
    let (_sandbox, a, _, _) = set_up();
    let p = a.run(["show", "--json"]);
    assert_eq!(p.returncode(), 2);
    assert_eq!(p.stdout, "");
    assert!(
        p.stderr.trim_start_matches(is_space).starts_with('{'),
        "parser errors bypass JSON error envelope: {}",
        p.stderr
    );
    assert!(parse_json(&p.stderr).unwrap().get("error").is_some());
}

#[test]
fn global_options_respect_end_of_options() {
    let (_sandbox, a, _, _) = set_up();
    let p = a.run(["create", "--", "--json"]);
    assert_eq!(
        p.returncode(),
        0,
        "literal --json title was consumed: {}",
        p.stderr
    );
    let listed = a.bead(["list"]).unwrap();
    let titles: Vec<Option<&str>> = listed["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str())
        .collect();
    assert!(titles.contains(&Some("--json")), "{titles:?}");
}

#[test]
fn claiming_update_validates_parent_before_committing_claim() {
    let (_sandbox, a, x, _) = set_up();
    let p = a.run(["update", &x, "--claim", "--parent", "missing", "--json"]);
    assert_ne!(p.returncode(), 0);
    assert!(
        a.show(&x).get("claim").is_none(),
        "failed update left an accepted online claim behind"
    );
}

#[test]
fn workspace_config_cannot_escape_home() {
    let (sandbox, a, _, _) = set_up();
    let absolute = sandbox.root().join("escaped-absolute");
    for wid in [absolute.to_str().unwrap(), "../escaped-relative"] {
        let config = dumps(&json!({ "workspace": wid }));
        std::fs::write(a.checkout().join(".beadroll.json"), config).unwrap();
        let p = a.run(["init", "--prefix", "evil", "--json"]);
        assert_ne!(
            p.returncode(),
            0,
            "{wid}: workspace id wrote outside BEAD_HOME: {}",
            p.stdout
        );
        assert_eq!(code(&p), "CONFIG", "{wid}");
    }
}

#[test]
fn changed_workspace_config_does_not_delete_old_store() {
    let (_sandbox, a, x, _) = set_up();
    let before = a.store();
    let saved = a.export();
    let config = a.checkout().join(".beadroll.json");
    let second = dumps(&json!({ "workspace": "second-workspace" }));
    std::fs::write(&config, second).unwrap();
    let p = a.run(["show", &x, "--json"]);
    assert_eq!(code(&p), "NOT_INITIALIZED");
    std::fs::remove_file(&config).unwrap();
    assert_eq!(a.store(), before);
    assert_eq!(a.export(), saved);
}

#[test]
fn import_preserves_dependency_metadata_extra_and_tombstone_mutation() {
    let (sandbox, a, x, _) = set_up();
    let src = sandbox.root().join("import.jsonl");
    let rec = json!({
        "id": "ex-z", "title": "imported", "status": "open", "priority": 2,
        "issue_type": "task", "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z", "created_by": "audit", "future": {"kept": true},
        "dependencies": [{
            "issue_id": "ex-z", "depends_on_id": (x.as_str()), "type": "blocks",
            "created_at": "2026-01-01T00:00:00Z", "created_by": "audit",
            "metadata": "{\"kept\":true}", "thread_id": "meta-thread"
        }]
    });
    std::fs::write(&src, dumps(&rec) + "\n").unwrap();
    a.bead(["import", src.to_str().unwrap()]);
    a.bead(["delete", "ex-z", "--reason", "duplicate"]);
    let got = record(&a.export(), "ex-z");
    assert_eq!(got["future"], json!({"kept": true}));
    assert_eq!(got["dependencies"], rec["dependencies"]);
    assert_eq!(got["status"], json!("tombstone"));
    assert_eq!(got["delete_reason"], json!("duplicate"));
}

#[test]
fn imported_comment_zero_id_roundtrips() {
    let (sandbox, a, _, _) = set_up();
    let src = sandbox.root().join("comment-zero.jsonl");
    let rec = json!({"id": "ex-z", "title": "zero comment", "comments": [{
        "id": 0, "author": "audit", "text": "zero", "created_at": "2026-01-01T00:00:00Z"
    }]});
    std::fs::write(&src, dumps(&rec) + "\n").unwrap();
    a.bead(["import", src.to_str().unwrap()]);
    let p = a.run(["export"]);
    assert_eq!(p.returncode(), 0, "{}", p.stderr);
    assert_eq!(record(&p.stdout, "ex-z")["comments"][0]["id"], json!(0));
}

#[test]
fn metadata_excludes_local_machine_identifiers() {
    let (sandbox, a, x, _) = set_up();
    a.bead(["comments", "add", &x, "synthetic comment"]);
    let forbidden = [
        python_home(),
        sandbox.root().to_str().unwrap().to_owned(),
        hostname(),
        format!("{}@", user()),
    ];
    for event in journal_events(&a.store()) {
        let mut metadata = event.as_object().unwrap().clone();
        metadata.shift_remove("data");
        let text = dumps(&Json::Object(metadata));
        for word in &forbidden {
            assert!(!text.contains(word.as_str()), "{word:?} in {text}");
        }
        let replica = event["replica"].as_str().unwrap();
        let hex = |b| matches!(b, b'0'..=b'9' | b'a'..=b'f');
        assert!(
            replica.len() == 8 && replica.bytes().all(hex),
            "replica {replica:?} is not 8 lowercase hex digits"
        );
    }
}

/// Every event in the store's journals, `events/*/*.jsonl`.
fn journal_events(store: &Path) -> Vec<Json> {
    let mut events = Vec::new();
    for replica in std::fs::read_dir(store.join("events")).unwrap() {
        let Ok(segments) = std::fs::read_dir(replica.unwrap().path()) else {
            continue; // not a directory
        };
        for segment in segments {
            let path = segment.unwrap().path();
            if path.to_str().unwrap().ends_with(".jsonl") {
                let text = std::fs::read_to_string(&path).unwrap();
                events.extend(text.lines().map(|line| parse_json(line).unwrap()));
            }
        }
    }
    events
}

/// The user's home as Python's `Path.home()` names it: `HOME`, else the password database's,
/// without a trailing slash.
fn python_home() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| {
        let user = nix::unistd::User::from_uid(nix::unistd::Uid::current());
        user.unwrap().unwrap().dir.to_str().unwrap().to_owned()
    });
    match home.trim_end_matches('/') {
        "" => "/".to_owned(),
        home => home.to_owned(),
    }
}

/// The machine's name, as `socket.gethostname()` gives it.
fn hostname() -> String {
    let name = rustix::system::uname();
    name.nodename().to_string_lossy().into_owned()
}

/// The user's login name, as `getpass.getuser()` finds it: the first of `LOGNAME`, `USER`, `LNAME`
/// and `USERNAME` that is set, else the password database's.
fn user() -> String {
    ["LOGNAME", "USER", "LNAME", "USERNAME"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| {
            let user = nix::unistd::User::from_uid(nix::unistd::Uid::current());
            user.unwrap().unwrap().name
        })
}
