//! `Review5CLI`: the fifth independent review's findings (2026-10-07-5-codex) at the command line,
//! folded in after the fixes: untrusted text stays inert, secrets stay private, a bad import or
//! configuration fails whole with a structured error, `--json` is always JSON, and the retry of a
//! lost acknowledgement whose file changed is a new request.

use std::collections::BTreeSet;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::process::Stdio;

use beadroll_core::json::{Format, Object, write};
use beadroll_test_support::{Json, Machine, Run, Sandbox, code, json, parse_json};

/// `machine`: a machine `name` whose tracker, prefix `ex`, is initialised.
fn machine(sandbox: &Sandbox, name: &str) -> Machine {
    let machine = Machine::new(sandbox, name);
    machine.bead(["init", "--prefix", "ex"]);
    machine
}

/// The `id` member of what `bead ... --json` printed.
fn id(result: Option<Json>) -> String {
    let result = result.expect("a result");
    result["id"].as_str().expect("an id").to_owned()
}

/// `strict_json`: `text` read as `json.loads` reads it with `NaN` and the infinities refused, or the
/// test fails. beadroll-core's reader refuses them, and numbers too large for a float as well.
fn strict_json(text: &str) -> Json {
    parse_json(text).unwrap_or_else(|error| panic!("not strict JSON ({error}): {text:?}"))
}

/// `COMMON`: the members every record these tests import shares.
fn common() -> Json {
    json!({
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed"
    })
}

/// `json.dumps({**head, **COMMON, **tail})`: the members in that order, a later value taking an
/// earlier one's place, written as `json.dumps` writes them (spaced, ASCII-escaped).
fn record(head: Json, tail: Json) -> String {
    let mut members = Object::new();
    for part in [head, common(), tail] {
        let Json::Object(part) = part else {
            panic!("members, not {part:?}");
        };
        // as a dict display: a member already there keeps its place and takes the new value
        members.extend(*part);
    }
    write(&Json::from(members), Format::ASCII).expect("a record")
}

/// The private-use character a record holds where Python's holds a lone surrogate, which no Rust
/// string can hold.
const STAND_IN: char = '\u{E000}';

/// E01: untrusted fields must not execute terminal protocols or forge lines.
#[test]
fn e01_human_output_escapes_terminal_controls() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e01-terminal");
    let title = "safe\x1b]52;c;SGVsbG8=\x07\nFORGED: success";
    let created = machine.run(["create", title]);
    assert_eq!(created.returncode(), 0, "{}", created.stderr);
    let listing = machine.run(["list"]);
    assert_eq!(listing.returncode(), 0, "{}", listing.stderr);
    for output in [&created.stdout, &listing.stdout] {
        assert!(!output.contains('\x1b'), "{output:?}");
        assert!(!output.contains('\x07'), "{output:?}");
        assert!(!output.contains("\nFORGED: success"), "{output:?}");
    }
}

/// E02: remote URL credentials must be redacted and protected at rest.
#[test]
fn e02_remote_credentials_are_not_disclosed_or_world_readable() {
    let sandbox = crate::sandbox();
    let secret = "supersecret";
    let remote = format!("https://alice:{secret}@example.invalid/private.git");
    let machine = Machine::new(&sandbox, "e02-secret");
    let initialized = machine.run(["init", "--prefix", "ex", "--remote", &remote, "--json"]);
    assert_eq!(initialized.returncode(), 0, "{}", initialized.stderr);
    let info = machine.run(["info", "--json"]);
    assert_eq!(info.returncode(), 0, "{}", info.stderr);
    assert!(!info.stdout.contains(secret), "{}", info.stdout);
    let meta = machine.store().join("meta.json");
    let text = std::fs::read_to_string(&meta).unwrap();
    assert!(!text.contains(secret), "{text}");
    // `stat.S_IMODE(mode) & 0o077`: no permission for the group or for others
    let mode = std::fs::metadata(&meta).unwrap().permissions().mode();
    assert_eq!(mode & 0o7777 & 0o077, 0, "meta.json has mode {mode:o}");
}

/// E03: a takeover must fence the prior owner's stale terminal transition.
#[test]
fn e03_takeover_fences_an_owners_concurrent_offline_delete() {
    let sandbox = crate::sandbox();
    let owner = machine(&sandbox, "e03-owner");
    let key = id(owner.bead(["create", "terminal race"]));
    owner.bead(["sync"]);
    let successor = Machine::new(&sandbox, "e03-successor");
    successor.bead(["sync"]);
    owner.bead(["claim", &key]);

    owner.bead(["delete", &key, "--offline"]);
    successor.bead(["takeover", &key, "--reason", "continue work"]);
    owner.bead(["sync"]);
    successor.bead(["sync"]);

    let shown = successor.show(&key);
    assert_eq!(shown["status"].as_str(), Some("in_progress"), "{shown:?}");
    // `shown.get("conflicts", {})`: no conflicts at all, or none about the status
    let conflicts = shown
        .get("conflicts")
        .map(|c| c.as_object().expect("conflicts by field"));
    assert!(
        !conflicts.is_some_and(|c| c.contains_key("status")),
        "{shown:?}"
    );
    let claimant = shown["claim"]["owner"].as_str().expect("a claim's owner");
    assert!(claimant.contains("e03-successor-thread"), "{claimant}");
}

/// E04: an import count and exit zero must mean the issue was imported.
#[test]
fn e04_invalid_import_fails_instead_of_counting_an_inert_event() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e04-import");
    let source = sandbox.root().join("invalid.jsonl");
    let invalid = record(
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

/// E05: every successful --json command must emit valid JSON.
#[test]
fn e05_silent_create_still_honors_json_mode() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e05-silent");
    let created = machine.run(["create", "machine result", "--silent", "--json"]);
    assert_eq!(created.returncode(), 0, "{}", created.stderr);
    let result = parse_json(&created.stdout).unwrap_or_else(|error| {
        panic!(
            "successful --json output was not JSON: {error}; bytes={:?}",
            created.stdout
        )
    });
    let members = result.as_object().unwrap_or_else(|| panic!("{result:?}"));
    assert!(members.contains_key("id"), "{result:?}");
}

/// E06: accepted data must not make Beads JSONL violate RFC 8259.
#[test]
fn e06_export_never_emits_nonfinite_json_numbers() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e06-json");
    let source = sandbox.root().join("nonfinite.jsonl");
    let nonfinite = concat!(
        r#"{"id":"ex-nan","title":"bad number","status":"open","priority":2,"#,
        r#""issue_type":"task","created_at":"2026-01-01T00:00:00Z","#,
        r#""updated_at":"2026-01-01T00:00:00Z","created_by":"seed","score":NaN}"#,
        "\n",
    );
    std::fs::write(&source, nonfinite).unwrap();
    let imported = machine.run(["import", &source.display().to_string(), "--json"]);
    if imported.returncode() != 0 {
        // refused, with an error that is itself strict JSON
        strict_json(&imported.stderr);
        return;
    }
    let exported = machine.run(["export"]);
    assert_eq!(exported.returncode(), 0, "{}", exported.stderr);
    // `splitlines()`: the one record here is ASCII, with none of the other breaks Python splits at
    for line in exported.stdout.lines() {
        if let Err(error) = parse_json(line) {
            panic!("export emitted non-RFC-8259 JSON: {error}; line={line:?}");
        }
    }
}

/// E07: an early pipeline consumer exit must not produce Python flush noise.
#[test]
fn e07_closed_downstream_pipe_has_defined_quiet_exit() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e07-pipe");
    let key = id(machine.bead(["create", "pipe target"]));
    let mut command = machine.cmd(["show", &key]).command();
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bead starts");
    // the reader of standard output is gone before bead writes to it
    drop(child.stdout.take());
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("bead's standard error")
        .read_to_end(&mut stderr)
        .expect("bead's standard error");
    let run = Run {
        status: child.wait().expect("bead ends"),
        stdout: String::new(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    };
    let returncode = run.returncode();
    assert!(
        [0, 141].contains(&returncode),
        "{returncode}: {}",
        run.stderr
    );
    assert!(!run.stderr.contains("BrokenPipeError"), "{}", run.stderr);
    assert!(!run.crashed(), "{}", run.stderr);
}

/// E08: values accepted by import validation must be safe in every query path.
#[test]
fn e08_nullable_import_values_do_not_crash_render_or_export() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e08-nullable");
    let comments = sandbox.root().join("nullable-comment.jsonl");
    let commented = record(
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
    let depending = record(
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
fn u09_a_record_without_a_creation_time_is_not_imported() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "u09-created");
    machine.bead(["create", "listed first"]);
    let source = sandbox.root().join("no-creation-time.jsonl");
    let untimed = record(
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
/// drops it (a known record, source_repo_path); every record is now refused for it, as a raw one
/// already was.
#[test]
fn u11_an_escaped_lone_surrogate_fails_the_import_wherever_it_is() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "u11-surrogate");
    let good = record(json!({"id": "ex-fine", "title": "fine"}), json!({}));
    // A Rust string cannot hold a lone surrogate: the record holds STAND_IN where Python's holds
    // one, and the stand-in's escape becomes the surrogate's, as `json.dumps` escapes it.
    let escape = |unit: u32| format!(r"\u{unit:04x}");
    let lone = |line: String, surrogate: u32| {
        let stand_in = escape(u32::from(STAND_IN));
        assert!(line.contains(&stand_in), "{line}");
        line.replace(&stand_in, &escape(surrogate))
    };
    let title = record(
        json!({"id": "ex-1"}),
        json!({"title": (format!("a{STAND_IN}"))}),
    );
    let dropped = record(
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
        let message = format!("{}:2: text is not valid UTF-8", source.display());
        assert_eq!(
            strict_json(&imported.stderr)["error"],
            json!({"code": "VALIDATION", "message": (message)}),
            "{name}"
        );
    }
    assert_eq!(machine.bead(["list"]).unwrap()["issues"], json!([]));
}

/// E09: malformed tracked config must not escape as Python tracebacks.
#[test]
fn e09_invalid_config_types_return_structured_config_errors() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "e09-config");
    let config = machine.checkout().join(".beadroll.json");
    let cases = [
        "[]\n",
        "{\"workspace\":\"safe\",\"ref\":42}\n",
        "{\"workspace\":\"safe\",\"remote\":{\"url\":\"x\"}}\n",
    ];
    for body in cases {
        std::fs::write(&config, body).unwrap();
        let result = machine.run(["info", "--json"]);
        assert_ne!(result.returncode(), 0, "config {body:?}");
        assert!(!result.crashed(), "config {body:?}: {}", result.stderr);
        assert_eq!(code(&result), "CONFIG", "config {body:?}");
    }
}

/// E11: --parent must match any parent represented in the issue's dependency set.
#[test]
fn e11_parent_filter_matches_every_direct_parent() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e11-parents");
    let first = id(machine.bead(["create", "parent one"]));
    let second = id(machine.bead(["create", "parent two"]));
    let child = id(machine.bead(["create", "child"]));
    machine.bead(["dep", "add", &child, &first, "--type", "parent-child"]);
    machine.bead(["dep", "add", &child, &second, "--type", "parent-child"]);
    for parent in [&first, &second] {
        let result = machine
            .bead(["list", "--all", "--parent", parent, "--limit", "0"])
            .unwrap();
        let issues = result["issues"].as_array().expect("issues");
        let ids: BTreeSet<&str> = issues
            .iter()
            .map(|issue| issue["id"].as_str().expect("an id"))
            .collect();
        assert!(ids.contains(child.as_str()), "parent {parent}: {ids:?}");
    }
}

/// E12: rendering in a reduced locale must not report failure after committing.
#[test]
fn e12_ascii_locale_cannot_turn_a_committed_create_into_failure() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e12-ascii");
    let result = machine
        .cmd(["create", "committed but failed"])
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("PYTHONUTF8", "0")
        .env("PYTHONCOERCECLOCALE", "0")
        .run();
    assert_eq!(result.returncode(), 0, "{}", result.stderr);
    assert!(!result.crashed(), "{}", result.stderr);
    let listed = machine.bead(["list", "--all", "--limit", "0"]).unwrap();
    let issues = listed["issues"].as_array().expect("issues");
    let titles: Vec<Option<&str>> = issues.iter().map(|i| i["title"].as_str()).collect();
    assert_eq!(titles, [Some("committed but failed")]);
}

/// E13: changed file input is a new semantic request after lost-ack recovery.
#[test]
fn e13_receipt_identity_includes_resolved_file_content() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e13-receipt-file");
    let key = id(machine.bead(["create", "receipt target"]));
    machine.bead(["sync"]);
    let body = sandbox.root().join("description.txt");
    let file = body.display().to_string();
    std::fs::write(&body, "first body").unwrap();
    let update = [
        "update",
        &key,
        "--claim",
        "--description-file",
        &file,
        "--json",
    ];
    let first = machine
        .cmd(update)
        .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
        .run();
    assert_eq!(first.returncode(), 99, "{}", first.stderr);
    std::fs::write(&body, "second body").unwrap();
    let retry = machine.run(update);
    assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    assert_eq!(
        machine.show(&key)["description"].as_str(),
        Some("second body")
    );
}

/// V01: the standard JSON serializer keeps control data inert and recoverable.
#[test]
fn v01_machine_json_preserves_and_escapes_control_data() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "v01-json-control");
    let title = "safe\x1b]52;c;SGVsbG8=\x07\nline";
    let result = machine.run(["create", title, "--json"]);
    assert_eq!(result.returncode(), 0, "{}", result.stderr);
    assert!(!result.stdout.contains('\x1b'), "{:?}", result.stdout);
    assert_eq!(strict_json(&result.stdout)["title"].as_str(), Some(title));
}
