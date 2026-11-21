//! Browsing ready issues (order, 25-row cap, empty state), full detail views (including
//! unconfirmed ownership attempts, lost/stale claims, closed_on/merged, multi-parent, and
//! tracked config warnings), local-only reads, and Refresh rereading local state without syncing.

use serde_json::{Value, json};
use tempfile::tempdir;

use crate::{McpClient, write_stateful_fake_bead};

fn sample_info(last_sync: Option<&str>) -> Value {
    json!({
        "version": "0.2.0",
        "workspace": "ws-alpha-0001",
        "workspace_source": "tracked .beadroll.json",
        "name": "alpha-tracker",
        "prefix": "alp",
        "last_sync": last_sync,
    })
}

#[test]
fn ready_preserves_order_caps_at_25_and_reports_more() {
    let dir = tempdir().expect("tempdir");
    let state_file = dir.path().join("state.json");
    let log_file = dir.path().join("bead.log");

    let ready: Vec<Value> = (1..=30)
        .map(|i| {
            json!({
                "id": format!("alp-{i:02}"),
                "uid": format!("019a0000-0000-7000-8000-{i:012}"),
                "title": format!("Task number {i}"),
                "status": "open",
                "priority": i % 5,
                "issue_type": "task",
                "created_at": "2026-11-20T10:00:00Z",
                "updated_at": "2026-11-20T10:00:00Z",
            })
        })
        .collect();
    std::fs::write(
        &state_file,
        serde_json::to_string(&json!({
            "info": sample_info(Some("2026-11-21T15:00:00Z")),
            "ready": ready,
            "details": {},
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

    let res = client.call_tool("bead_ready", json!({}));
    assert_ne!(res.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        res.pointer("/structuredContent/view")
            .and_then(Value::as_str),
        Some("ready")
    );
    assert_eq!(
        res.pointer("/structuredContent/has_more")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        res.pointer("/structuredContent/workspace/id")
            .and_then(Value::as_str),
        Some("ws-alpha-0001")
    );
    assert_eq!(
        res.pointer("/structuredContent/workspace/last_sync")
            .and_then(Value::as_str),
        Some("2026-11-21T15:00:00Z")
    );
    let issues = res
        .pointer("/structuredContent/issues")
        .and_then(Value::as_array)
        .expect("issues array");
    assert_eq!(issues.len(), 25);
    let ids: Vec<&str> = issues
        .iter()
        .filter_map(|i| i.get("id").and_then(Value::as_str))
        .collect();
    let expected: Vec<String> = (1..=25).map(|i| format!("alp-{i:02}")).collect();
    assert_eq!(ids, expected);

    let text = res
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .expect("text");
    assert!(text.contains("alpha-tracker"));
    assert!(text.contains("2026-11-21T15:00:00Z"));
    assert!(text.contains("alp-01"));
    assert!(text.contains("alp-25"));
    assert!(!text.contains("alp-26"));

    let log = std::fs::read_to_string(&log_file).expect("read log");
    for line in log.lines() {
        let entry: Value = serde_json::from_str(line).expect("log line");
        let cmd = entry
            .pointer("/argv/0")
            .and_then(Value::as_str)
            .expect("cmd");
        assert!(
            matches!(cmd, "info" | "ready"),
            "unexpected bead command: {cmd}"
        );
    }

    assert_eq!(client.close(), Some(0));
}

#[test]
fn ready_empty_state_and_refresh_rereads_local_tracker() {
    let dir = tempdir().expect("tempdir");
    let state_file = dir.path().join("state.json");
    let log_file = dir.path().join("bead.log");

    std::fs::write(
        &state_file,
        serde_json::to_string(&json!({
            "info": sample_info(None),
            "ready": [],
            "details": {},
        }))
        .expect("json"),
    )
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

    let empty = client.call_tool("bead_ready", json!({}));
    assert_eq!(
        empty
            .pointer("/structuredContent/issues")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        empty
            .pointer("/structuredContent/has_more")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert!(
        empty
            .pointer("/structuredContent/workspace/last_sync")
            .is_some_and(Value::is_null)
    );
    let empty_text = empty
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .expect("text");
    assert!(empty_text.contains("No ready issues"));

    std::fs::write(
        &state_file,
        serde_json::to_string(&json!({
            "info": sample_info(Some("2026-11-21T18:30:00Z")),
            "ready": [{
                "id": "alp-new",
                "uid": "019a0000-0000-7000-8000-000000000099",
                "title": "Freshly created issue",
                "status": "open",
                "priority": 1,
                "issue_type": "bug",
            }],
            "details": {},
        }))
        .expect("json"),
    )
    .expect("write updated state");

    let refreshed = client.call_tool("bead_ready", json!({}));
    assert_eq!(
        refreshed
            .pointer("/structuredContent/workspace/last_sync")
            .and_then(Value::as_str),
        Some("2026-11-21T18:30:00Z")
    );
    assert_eq!(
        refreshed
            .pointer("/structuredContent/issues/0/id")
            .and_then(Value::as_str),
        Some("alp-new")
    );

    assert_eq!(client.close(), Some(0));
}

fn sample_detail_fixture(uid: &str, dep_uid: &str, parent_uid: &str) -> Value {
    json!({
        "info": sample_info(Some("2026-11-21T16:00:00Z")),
        "ready": [],
        "details": {
            "alp-42": {
                "id": "alp-42",
                "uid": uid,
                "title": "Harden rate limiter",
                "description": "Keep token counts across restarts.",
                "acceptance_criteria": "Restart preserves remaining quota.",
                "external_ref": "GH-102",
                "status": "in_progress",
                "priority": 1,
                "issue_type": "bug",
                "labels": ["backend", "urgent"],
                "parent": "alp-1",
                "parent_uid": parent_uid,
                "parents": ["alp-1", "alp-2"],
                "parent_uids": [parent_uid, "019a0000-0000-7000-8000-000000000002"],
                "blocked": false,
                "created_at": "2026-11-20T09:00:00Z",
                "created_by": "codex:t1@aaaa1111",
                "updated_at": "2026-11-21T11:00:00Z",
                "where": { "branch": "fix/rate-limit", "commit": "abcdef1234567890" },
                "claim": {
                    "owner": "claude:s2@bbbb2222",
                    "since": "2026-11-21T10:30:00Z",
                    "generation": 2,
                    "where": { "branch": "fix/rate-limit", "commit": "abcdef123456" }
                },
                "dependencies": [{
                    "id": "alp-10",
                    "uid": dep_uid,
                    "title": "Persist bucket state",
                    "status": "closed",
                    "dependency_type": "blocks"
                }],
                "unresolved_dependencies": [
                    { "id": "legacy-9", "dependency_type": "blocks" }
                ],
                "dependents": [{
                    "id": "alp-50",
                    "uid": "019a0000-0000-7000-8000-000000000050",
                    "title": "Ship release",
                    "status": "open",
                    "dependency_type": "blocks"
                }],
                "comments": [{
                    "id": "c-1",
                    "author": "claude:s2@bbbb2222",
                    "text": "Reproduced with two workers.",
                    "created_at": "2026-11-21T10:45:00Z",
                    "branch": "fix/rate-limit"
                }]
            }
        }
    })
}

#[test]
fn show_returns_full_detail_sections_and_readable_text_fallback() {
    let dir = tempdir().expect("tempdir");
    let state_file = dir.path().join("state.json");
    let log_file = dir.path().join("bead.log");

    let uid = "019a0000-0000-7000-8000-000000000042";
    let dep_uid = "019a0000-0000-7000-8000-000000000010";
    let parent_uid = "019a0000-0000-7000-8000-000000000001";
    let fixture = sample_detail_fixture(uid, dep_uid, parent_uid);
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

    let res = client.call_tool("bead_show", json!({ "id": "alp-42" }));
    assert_ne!(res.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        res.pointer("/structuredContent/view")
            .and_then(Value::as_str),
        Some("detail")
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/uid")
            .and_then(Value::as_str),
        Some(uid)
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/parents"),
        Some(&json!(["alp-1", "alp-2"]))
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/external_ref")
            .and_then(Value::as_str),
        Some("GH-102")
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/claim/owner")
            .and_then(Value::as_str),
        Some("claude:s2@bbbb2222")
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/dependencies/0/uid")
            .and_then(Value::as_str),
        Some(dep_uid)
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/comments/0/text")
            .and_then(Value::as_str),
        Some("Reproduced with two workers.")
    );

    let text = res
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .expect("text");
    for needle in [
        "Harden rate limiter",
        "Keep token counts across restarts.",
        "Restart preserves remaining quota.",
        "GH-102",
        "alp-1, alp-2",
        "claude:s2@bbbb2222",
        "alp-10",
        "Reproduced with two workers.",
    ] {
        assert!(text.contains(needle), "fallback text missing {needle:?}");
    }

    assert_eq!(client.close(), Some(0));
}

fn ownership_and_closure_fixture() -> Value {
    let mut info = sample_info(Some("2026-11-21T16:30:00Z"));
    info["tracked_config_warning"] = json!(".beadroll.json is untracked on this branch");
    json!({
        "info": info,
        "ready": [],
        "details": {
            "alp-9": {
                "id": "alp-9",
                "uid": "019a0000-0000-7000-8000-000000000009",
                "title": "Ownership uncertainty check",
                "status": "closed",
                "priority": 2,
                "issue_type": "task",
                "closed_at": "2026-11-21T16:20:00Z",
                "close_reason": "Landed on topic branch",
                "estimated_minutes": 45,
                "where": { "branch": null, "commit": "abcdef1234567890" },
                "closed_on": { "branch": "feat/topic", "commit": "1234567890abcdef" },
                "merged": false,
                "unconfirmed": [{
                    "event": "019a0000-0000-7000-8000-000000000901",
                    "op": "claim",
                    "by": "codex:t9@cccc3333",
                    "at": "2026-11-21T16:15:00Z"
                }],
                "lost_claims": [{
                    "event": "019a0000-0000-7000-8000-000000000902",
                    "by": "claude:s1@dddd4444",
                    "held_by": "codex:t9@cccc3333",
                    "at": "2026-11-21T16:16:00Z"
                }],
                "stale": [{
                    "op": "release",
                    "by": "claude:s1@dddd4444",
                    "at": "2026-11-21T16:17:00Z",
                    "claim": "019a0000-0000-7000-8000-000000000900"
                }],
                "dependencies": [],
                "dependents": [],
                "comments": [],
                "blocked": false,
            }
        }
    })
}

#[test]
fn show_preserves_unconfirmed_lost_stale_and_closure_metadata() {
    let dir = tempdir().expect("tempdir");
    let state_file = dir.path().join("state.json");
    let log_file = dir.path().join("bead.log");

    let fixture = ownership_and_closure_fixture();
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

    let res = client.call_tool("bead_show", json!({ "id": "alp-9" }));
    assert_eq!(
        res.pointer("/structuredContent/workspace/tracked_config_warning")
            .and_then(Value::as_str),
        Some(".beadroll.json is untracked on this branch")
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/unconfirmed/0/event")
            .and_then(Value::as_str),
        Some("019a0000-0000-7000-8000-000000000901")
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/lost_claims/0/at")
            .and_then(Value::as_str),
        Some("2026-11-21T16:16:00Z")
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/stale/0/claim")
            .and_then(Value::as_str),
        Some("019a0000-0000-7000-8000-000000000900")
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/estimated_minutes")
            .and_then(Value::as_i64),
        Some(45)
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/closed_on_branch")
            .and_then(Value::as_str),
        Some("feat/topic")
    );
    assert_eq!(
        res.pointer("/structuredContent/issue/merged")
            .and_then(Value::as_bool),
        Some(false)
    );

    let text = res
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .expect("text");
    for needle in [
        "untracked on this branch",
        "Estimated minutes: 45",
        "Last worked on branch: detached HEAD (abcdef123456)",
        "Unconfirmed: claim by codex:t9@cccc3333",
        "Lost claim: claude:s1@dddd4444 lost to codex:t9@cccc3333",
        "Ignored stale release by claude:s1@dddd4444",
        "Closed on: feat/topic at 1234567890ab",
        "NOT yet on the default branch",
    ] {
        assert!(text.contains(needle), "fallback text missing {needle:?}");
    }

    assert_eq!(client.close(), Some(0));
}
