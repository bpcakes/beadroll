//! Contract and security invariants between the Rust server's `structuredContent` JSON and the
//! bundled MCP App view in `ui/index.html`.

use beadroll_mcp::UI_HTML;
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::{McpClient, write_stateful_fake_bead};

const WORKSPACE_KEYS: [&str; 8] = [
    "id",
    "prefix",
    "name",
    "source",
    "last_sync",
    "bead_version",
    "tracked_config_warning",
    "checkout",
];

const DETAIL_ISSUE_KEYS: [&str; 38] = [
    "id",
    "uid",
    "title",
    "status",
    "priority",
    "issue_type",
    "description",
    "acceptance_criteria",
    "notes",
    "design",
    "external_ref",
    "close_reason",
    "assignee",
    "labels",
    "parent",
    "parent_uid",
    "parents",
    "parent_uids",
    "blocked",
    "created_at",
    "created_by",
    "updated_at",
    "closed_at",
    "where_branch",
    "where_commit",
    "closed_on_branch",
    "closed_on_commit",
    "merged",
    "claim",
    "provisional_claims",
    "unconfirmed",
    "lost_claims",
    "stale",
    "dependencies",
    "unresolved_dependencies",
    "dependents",
    "comments",
    "conflicts",
];

#[test]
fn ui_html_enforces_csp_xss_accessibility_and_protocol_invariants() {
    assert!(UI_HTML.starts_with("<!DOCTYPE html>"));

    // Strict default CSP: no external URLs, no dangerous DOM sinks.
    for forbidden in [
        "http://",
        "https://",
        "innerHTML",
        "outerHTML",
        "insertAdjacentHTML",
        "document.write",
    ] {
        assert!(
            !UI_HTML.contains(forbidden),
            "ui/index.html must not contain {forbidden:?}"
        );
    }

    // Dedicated polite live region outside `#app`; `#app` itself must not be `aria-live`.
    assert!(UI_HTML.contains(r#"<div id="live-status" class="sr-only" aria-live="polite""#));
    assert!(UI_HTML.contains(r#"<div id="app" role="main"></div>"#));

    // Required MCP Apps postMessage protocol handlers & sizing rules.
    for required in [
        "ui/initialize",
        "ui/notifications/initialized",
        "ui/notifications/tool-input",
        "ui/notifications/tool-result",
        "ui/notifications/tool-cancelled",
        "ui/notifications/host-context-changed",
        "ui/notifications/size-changed",
        "ui/request-display-mode",
        "ui/update-model-context",
        "ui/resource-teardown",
        "sendErrorResponse(msg.id, -32601",
        "sendNotification(\"ui/notifications/size-changed\", { height })",
        "INIT_FAILED",
    ] {
        assert!(
            UI_HTML.contains(required),
            "ui/index.html missing protocol hook {required:?}"
        );
    }
}

#[test]
fn structured_content_keys_match_every_field_consumed_by_ui() {
    let dir = tempdir().expect("tempdir");
    let state_file = dir.path().join("state.json");
    let log_file = dir.path().join("bead.log");

    let uid = "019a0000-0000-7000-8000-000000000077";
    let fixture = json!({
        "info": {
            "version": "0.2.0",
            "workspace": "ws-contract",
            "workspace_source": "origin url",
            "name": "contract",
            "prefix": "con",
            "last_sync": "2026-11-21T12:00:00Z",
            "tracked_config_warning": "note"
        },
        "ready": [{
            "id": "con-1",
            "uid": uid,
            "title": "Contract row",
            "status": "open",
            "priority": 1,
            "issue_type": "task",
            "labels": ["a"],
            "conflicted": []
        }],
        "details": {
            "con-1": {
                "id": "con-1",
                "uid": uid,
                "title": "Contract row",
                "status": "open",
                "priority": 1,
                "issue_type": "task",
                "dependencies": [],
                "dependents": [],
                "comments": [],
                "blocked": false
            }
        }
    });
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

    let ready = client.call_tool("bead_ready", json!({}));
    let ready_ws = ready
        .pointer("/structuredContent/workspace")
        .and_then(Value::as_object)
        .expect("ready workspace");
    for key in WORKSPACE_KEYS {
        assert!(ready_ws.contains_key(key), "workspace missing key {key:?}");
    }

    let detail = client.call_tool(
        "bead_show",
        json!({ "uid": uid, "workspace": "ws-contract" }),
    );
    let issue = detail
        .pointer("/structuredContent/issue")
        .and_then(Value::as_object)
        .expect("detail issue");
    for key in DETAIL_ISSUE_KEYS {
        assert!(issue.contains_key(key), "issue detail missing key {key:?}");
    }

    assert_eq!(client.close(), Some(0));
}
