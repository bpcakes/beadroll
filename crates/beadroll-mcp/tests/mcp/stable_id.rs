//! Stable UID reopening when display keys change after list render, explicit UID collision
//! rejection, actionable upgrade errors on older `bead` builds, missing binary, uninitialized
//! tracker, dash-prefixed target safety, and subprocess timeout cleanup.

use serde_json::{Value, json};
use tempfile::tempdir;

use crate::{McpClient, write_fake_bead, write_stateful_fake_bead};

fn rename_state(last_sync: &str, ready: Value, details: Value) -> Value {
    json!({
        "info": {
            "version": "0.2.0",
            "workspace": "ws-rename",
            "name": "rename-test",
            "prefix": "ex",
            "last_sync": last_sync
        },
        "ready": ready,
        "details": details
    })
}

fn detail_row(id: &str, uid: &str, title: &str, description: &str) -> Value {
    json!({
        "id": id,
        "uid": uid,
        "title": title,
        "description": description,
        "status": "open",
        "priority": 1,
        "issue_type": "task",
        "dependencies": [],
        "dependents": [],
        "comments": [],
        "blocked": false,
    })
}

#[test]
fn clicking_row_reopens_renamed_issue_by_stable_uid_not_reassigned_key() {
    let dir = tempdir().expect("tempdir");
    let state_file = dir.path().join("state.json");
    let log_file = dir.path().join("bead.log");

    let original_uid = "019a0000-0000-7000-8000-111111111111";
    let usurper_uid = "019a0000-0000-7000-8000-222222222222";

    let initial = rename_state(
        "2026-11-21T10:00:00Z",
        json!([{
            "id": "ex-a.1",
            "uid": original_uid,
            "title": "Original child task",
            "status": "open",
            "priority": 1,
            "issue_type": "task",
        }]),
        json!({}),
    );
    std::fs::write(&state_file, serde_json::to_string(&initial).expect("json"))
        .expect("write initial state");

    let fake_bead = write_stateful_fake_bead(dir.path(), "fake-bead", &state_file, &log_file);
    let mut client = McpClient::spawn(
        dir.path(),
        &[
            "--repo",
            dir.path().to_str().expect("utf8"),
            "--bead",
            fake_bead.to_str().expect("utf8"),
        ],
        &[],
    );
    client.initialize();

    let listed = client.call_tool("bead_ready", json!({}));
    let row_uid = listed
        .pointer("/structuredContent/issues/0/uid")
        .and_then(Value::as_str)
        .expect("uid")
        .to_owned();
    let ws_id = listed
        .pointer("/structuredContent/workspace/id")
        .and_then(Value::as_str)
        .expect("ws_id")
        .to_owned();
    assert_eq!(row_uid, original_uid);

    // Sync renumbers the original issue to `ex-a.2`, and another issue now owns `ex-a.1`.
    let post_sync = rename_state(
        "2026-11-21T10:05:00Z",
        json!([]),
        json!({
            "ex-a.1": detail_row("ex-a.1", usurper_uid, "Different issue that took ex-a.1", ""),
            "ex-a.2": detail_row(
                "ex-a.2",
                original_uid,
                "Original child task",
                "This is the right issue after renumbering.",
            ),
        }),
    );
    std::fs::write(
        &state_file,
        serde_json::to_string(&post_sync).expect("json"),
    )
    .expect("write post-sync state");

    let reopened = client.call_tool(
        "bead_show",
        json!({ "id": "ex-a.1", "uid": row_uid, "workspace": ws_id }),
    );
    assert_ne!(reopened.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        reopened
            .pointer("/structuredContent/issue/id")
            .and_then(Value::as_str),
        Some("ex-a.2")
    );
    assert_eq!(
        reopened
            .pointer("/structuredContent/issue/uid")
            .and_then(Value::as_str),
        Some(original_uid)
    );
    assert_eq!(
        reopened
            .pointer("/structuredContent/issue/title")
            .and_then(Value::as_str),
        Some("Original child task")
    );

    assert_eq!(client.close(), Some(0));
}

#[test]
fn explicit_uid_lookup_refuses_display_key_colliding_with_that_uid() {
    let dir = tempdir().expect("tempdir");
    let state_file = dir.path().join("state.json");
    let log_file = dir.path().join("bead.log");

    let target_uid = "019a0000-0000-7000-8000-333333333333";
    let impostor_uid = "019a0000-0000-7000-8000-444444444444";

    // An imported issue's display key literally equals `target_uid`; because `bead show` resolves
    // keys before UIDs, it returns the impostor. `beadroll-mcp` must refuse the mismatch.
    let fixture = rename_state(
        "2026-11-21T11:00:00Z",
        json!([]),
        json!({
            "impostor": detail_row(target_uid, impostor_uid, "Imported key colliding with uid", ""),
            "real": detail_row("ex-real", target_uid, "Real target issue", ""),
        }),
    );
    std::fs::write(&state_file, serde_json::to_string(&fixture).expect("json"))
        .expect("write state");

    let fake_bead = write_stateful_fake_bead(dir.path(), "fake-bead", &state_file, &log_file);
    let mut client = McpClient::spawn(
        dir.path(),
        &[
            "--repo",
            dir.path().to_str().expect("utf8"),
            "--bead",
            fake_bead.to_str().expect("utf8"),
        ],
        &[],
    );
    client.initialize();

    let res = client.call_tool(
        "bead_show",
        json!({ "uid": target_uid, "workspace": "ws-rename" }),
    );
    assert_eq!(res.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        res.pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("UID_MISMATCH")
    );

    assert_eq!(client.close(), Some(0));
}

#[test]
fn older_bead_without_uid_produces_actionable_upgrade_error() {
    let dir = tempdir().expect("tempdir");
    let state_file = dir.path().join("state.json");
    let log_file = dir.path().join("bead.log");

    std::fs::write(
        &state_file,
        serde_json::to_string(&json!({
            "info": {
                "version": "0.2.0",
                "workspace": "ws-old",
                "name": "legacy-tracker",
                "prefix": "leg",
                "last_sync": "2026-11-20T00:00:00Z"
            },
            "ready": [{
                "id": "leg-1",
                "title": "No uid field emitted by older bead",
                "status": "open",
                "priority": 2,
                "issue_type": "task",
            }],
            "details": {
                "leg-1": {
                    "id": "leg-1",
                    "title": "No uid field emitted by older bead",
                    "status": "open",
                    "priority": 2,
                    "issue_type": "task",
                    "dependencies": [],
                    "dependents": [],
                    "comments": [],
                    "blocked": false,
                }
            }
        }))
        .expect("json"),
    )
    .expect("write state");

    let fake_bead = write_stateful_fake_bead(dir.path(), "fake-bead", &state_file, &log_file);
    let mut client = McpClient::spawn(
        dir.path(),
        &[
            "--repo",
            dir.path().to_str().expect("utf8"),
            "--bead",
            fake_bead.to_str().expect("utf8"),
        ],
        &[],
    );
    client.initialize();

    for (tool, args) in [
        ("bead_ready", json!({})),
        ("bead_show", json!({ "id": "leg-1" })),
    ] {
        let res = client.call_tool(tool, args);
        assert_eq!(res.get("isError").and_then(Value::as_bool), Some(true));
        assert_eq!(
            res.pointer("/structuredContent/error/code")
                .and_then(Value::as_str),
            Some("UPGRADE_REQUIRED")
        );
        let msg = res
            .pointer("/structuredContent/error/message")
            .and_then(Value::as_str)
            .expect("message");
        assert!(msg.contains("0.2.0"), "expected version in: {msg}");
        assert!(
            msg.contains("Upgrade"),
            "expected upgrade guidance in: {msg}"
        );
    }

    assert_eq!(client.close(), Some(0));
}

#[test]
fn missing_binary_uninitialized_tracker_and_not_found_surface_actionable_errors() {
    let dir = tempdir().expect("tempdir");

    let missing_bin = dir.path().join("no-such-bead-binary");
    let mut missing_client = McpClient::spawn(
        dir.path(),
        &[
            "--repo",
            dir.path().to_str().expect("utf8"),
            "--bead",
            missing_bin.to_str().expect("utf8"),
        ],
        &[],
    );
    missing_client.initialize();
    let missing_res = missing_client.call_tool("bead_ready", json!({}));
    assert_eq!(
        missing_res.get("isError").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        missing_res
            .pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("BEAD_NOT_FOUND")
    );
    assert_eq!(missing_client.close(), Some(0));

    let state_file = dir.path().join("state.json");
    let log_file = dir.path().join("bead.log");
    std::fs::write(
        &state_file,
        serde_json::to_string(&json!({
            "info": {
                "version": "0.2.0",
                "workspace": "local-uninit",
                "name": "scratch",
                "prefix": null,
                "last_sync": null
            },
            "ready_error": {
                "code": "NOT_INITIALIZED",
                "message": "no tracker for this repository here yet: run `bead sync` or `bead init`"
            },
            "details": {}
        }))
        .expect("json"),
    )
    .expect("write state");

    let fake_bead = write_stateful_fake_bead(dir.path(), "fake-bead", &state_file, &log_file);
    let mut client = McpClient::spawn(
        dir.path(),
        &[
            "--repo",
            dir.path().to_str().expect("utf8"),
            "--bead",
            fake_bead.to_str().expect("utf8"),
        ],
        &[],
    );
    client.initialize();

    let uninit = client.call_tool("bead_ready", json!({}));
    assert_eq!(uninit.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        uninit
            .pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("NOT_INITIALIZED")
    );

    let dash = client.call_tool("bead_show", json!({ "id": "--help" }));
    assert_eq!(dash.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        dash.pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("ISSUE_NOT_FOUND")
    );
    let log = std::fs::read_to_string(&log_file).expect("read log");
    let show_line = log
        .lines()
        .find(|line| line.contains(r#""show""#))
        .expect("show logged");
    let show_entry: Value = serde_json::from_str(show_line).expect("json");
    assert_eq!(
        show_entry.get("argv"),
        Some(&json!(["show", "--json", "--", "--help"]))
    );

    assert_eq!(client.close(), Some(0));
}

#[test]
fn stalled_bead_and_escaped_session_descendant_time_out_promptly() {
    let dir = tempdir().expect("tempdir");
    // Spawn a descendant in a brand-new session (`start_new_session=True`) holding stdout open
    // while the direct child exits immediately; nonblocking poll must time out and return
    // promptly without blocking on the escaped descendant's open pipe.
    let escaped_bead = write_fake_bead(
        dir.path(),
        "escaped-bead",
        r#"import subprocess, sys
subprocess.Popen(["sleep", "5"], stdout=sys.stdout, stderr=sys.stderr, start_new_session=True)
sys.exit(0)
"#,
    );

    let mut client = McpClient::spawn(
        dir.path(),
        &[
            "--repo",
            dir.path().to_str().expect("utf8"),
            "--bead",
            escaped_bead.to_str().expect("utf8"),
            "--timeout-ms",
            "250",
        ],
        &[],
    );
    client.initialize();

    let started = std::time::Instant::now();
    let res = client.call_tool("bead_ready", json!({}));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "timeout blocked on escaped descendant pipe: {:?}",
        started.elapsed()
    );
    assert_eq!(res.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        res.pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("BEAD_TIMEOUT")
    );

    let ping = client.request("ping", json!({}));
    assert_eq!(ping.get("result"), Some(&json!({})));

    assert_eq!(client.close(), Some(0));
}

#[test]
fn oversized_bead_output_returns_output_too_large_immediately_instead_of_timeout() {
    let dir = tempdir().expect("tempdir");
    // Write > 16 MiB to stdout and then sleep 30s; the nonblocking supervisor must detect the
    // overflow immediately, terminate the process group, and return BEAD_OUTPUT_TOO_LARGE.
    let noisy_bead = write_fake_bead(
        dir.path(),
        "noisy-bead",
        r#"import sys, time
chunk = b"x" * (1024 * 1024)
try:
    for _ in range(18):
        sys.stdout.buffer.write(chunk)
        sys.stdout.buffer.flush()
    time.sleep(30)
except BrokenPipeError:
    pass
"#,
    );

    let mut client = McpClient::spawn(
        dir.path(),
        &[
            "--repo",
            dir.path().to_str().expect("utf8"),
            "--bead",
            noisy_bead.to_str().expect("utf8"),
            "--timeout-ms",
            "10000",
        ],
        &[],
    );
    client.initialize();

    let started = std::time::Instant::now();
    let res = client.call_tool("bead_ready", json!({}));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "overflow waited for timeout instead of terminating early: {:?}",
        started.elapsed()
    );
    assert_eq!(res.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        res.pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("BEAD_OUTPUT_TOO_LARGE")
    );

    let ping = client.request("ping", json!({}));
    assert_eq!(ping.get("result"), Some(&json!({})));

    assert_eq!(client.close(), Some(0));
}
