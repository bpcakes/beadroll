//! Bidirectional contract and security invariants between the Rust server's `structuredContent`
//! JSON and the bundled MCP App view in `ui/index.html`.

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

const DETAIL_ISSUE_KEYS: [&str; 39] = [
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
    "estimated_minutes",
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

/// Extracts identifier tokens immediately following `<prefix>.` in `source` (excluding occurrences
/// embedded inside a longer identifier such as `my_issue.foo`).
fn referenced_properties<'a>(source: &'a str, prefix: &str) -> Vec<&'a str> {
    let needle = format!("{prefix}.");
    let mut found = Vec::new();
    let bytes = source.as_bytes();
    let mut start = 0;
    while let Some(rel) = source[start..].find(&needle) {
        let at = start + rel;
        let preceded_by_ident = at > 0
            && bytes
                .get(at - 1)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$');
        let prop_start = at + needle.len();
        if !preceded_by_ident {
            let prop_end = source[prop_start..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(source.len(), |offset| prop_start + offset);
            if prop_end > prop_start {
                found.push(&source[prop_start..prop_end]);
            }
        }
        start = prop_start;
    }
    found.sort_unstable();
    found.dedup();
    found
}

#[test]
fn ui_html_enforces_csp_xss_accessibility_version_and_protocol_invariants() {
    assert!(UI_HTML.starts_with("<!DOCTYPE html>"));

    // Keep `APP_INFO.version` aligned with the Cargo workspace version.
    let expected_version = format!(r#"version: "{}""#, env!("CARGO_PKG_VERSION"));
    assert!(
        UI_HTML.contains(&expected_version),
        "ui/index.html APP_INFO version drifted from Cargo.toml ({expected_version})"
    );

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

    assert!(UI_HTML.contains(r#"<div id="live-status" class="sr-only" aria-live="polite""#));
    assert!(UI_HTML.contains(r#"<div id="app" role="main"></div>"#));

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
fn structured_content_keys_match_ui_references_in_both_directions() {
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

    // Reverse direction: every `ws.<prop>` and `issue.<prop>` read in `ui/index.html` must exist
    // in the emitted JSON schema (allowing ready-row's `conflicted` key too).
    let ws_refs = referenced_properties(UI_HTML, "ws");
    assert!(!ws_refs.is_empty());
    for prop in ws_refs {
        assert!(
            ready_ws.contains_key(prop),
            "ui/index.html reads ws.{prop}, which server does not emit"
        );
    }

    let issue_refs = referenced_properties(UI_HTML, "issue");
    assert!(!issue_refs.is_empty());
    for prop in issue_refs {
        assert!(
            issue.contains_key(prop) || prop == "conflicted",
            "ui/index.html reads issue.{prop}, which server does not emit"
        );
    }

    assert_eq!(client.close(), Some(0));
}
