//! MCP JSON-RPC 2.0 handshake, tool/resource declarations, UI HTML invariants, and error framing.

use beadroll_mcp::{UI_RESOURCE_MIME_TYPE, UI_RESOURCE_URI};
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::McpClient;

#[test]
fn initialize_negotiates_supported_versions_and_survives_unknown_discover_method() {
    let dir = tempdir().expect("tempdir");
    let mut client = McpClient::spawn(dir.path(), &[], &[]);

    // Claude Code 2.1.330 probes `server/discover` before `initialize`; server must answer -32601
    // and stay alive for the standard `initialize` handshake.
    let discover = client.request("server/discover", json!({}));
    assert_eq!(
        discover.pointer("/error/code").and_then(Value::as_i64),
        Some(-32601)
    );

    let init = client.request(
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "codex-mcp-client", "version": "0.171.0" }
        }),
    );
    assert_eq!(
        init.pointer("/result/protocolVersion")
            .and_then(Value::as_str),
        Some("2025-06-18")
    );
    assert_eq!(
        init.pointer("/result/serverInfo/name")
            .and_then(Value::as_str),
        Some("beadroll-mcp")
    );
    assert_eq!(
        init.pointer("/result/capabilities/extensions/io.modelcontextprotocol~1ui/mimeTypes/0")
            .and_then(Value::as_str),
        Some(UI_RESOURCE_MIME_TYPE)
    );

    // Unknown protocol version falls back cleanly to latest supported version.
    let fallback = client.request(
        "initialize",
        json!({
            "protocolVersion": "1999-01-01",
            "capabilities": {},
            "clientInfo": { "name": "future", "version": "0.1.0" }
        }),
    );
    assert_eq!(
        fallback
            .pointer("/result/protocolVersion")
            .and_then(Value::as_str),
        Some("2025-11-25")
    );

    // Notifications produce no response, and ping still answers immediately.
    client.notify("notifications/initialized", json!({}));
    client.notify("notifications/cancelled", json!({ "requestId": 999 }));
    let ping = client.request("ping", json!({}));
    assert_eq!(ping.get("result"), Some(&json!({})));

    assert_eq!(client.close(), Some(0));
}

#[test]
fn tools_declare_read_only_mcp_app_contract_without_client_ui_capability() {
    let dir = tempdir().expect("tempdir");
    let mut client = McpClient::spawn(dir.path(), &[], &[]);
    // Client advertises NO `io.modelcontextprotocol/ui` extension (as Claude Code does); tools
    // must still expose `_meta.ui.resourceUri` so stock T3 Code renders the app.
    client.initialize();

    let tools_resp = client.request("tools/list", json!({}));
    let tools = tools_resp
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .expect("tools array");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();
    assert_eq!(names, ["bead_ready", "bead_show"]);

    for tool in tools {
        assert_eq!(
            tool.pointer("/annotations/readOnlyHint")
                .and_then(Value::as_bool),
            Some(true),
            "every tool must be marked readOnlyHint so T3 skips approval prompts"
        );
        assert_eq!(
            tool.pointer("/_meta/ui/resourceUri")
                .and_then(Value::as_str),
            Some(UI_RESOURCE_URI)
        );
        assert_eq!(
            tool.pointer("/_meta/ui/visibility"),
            Some(&json!(["model", "app"]))
        );
        assert_eq!(
            tool.pointer("/inputSchema/additionalProperties")
                .and_then(Value::as_bool),
            Some(false)
        );
        let props = tool
            .pointer("/inputSchema/properties")
            .and_then(Value::as_object)
            .expect("properties object");
        assert!(!props.contains_key("repo"));
        assert!(!props.contains_key("cwd"));
        assert!(!props.contains_key("path"));
    }

    assert_eq!(client.close(), Some(0));
}

#[test]
fn ui_resource_serves_self_contained_mcp_app_html() {
    let dir = tempdir().expect("tempdir");
    let mut client = McpClient::spawn(dir.path(), &[], &[]);
    client.initialize();

    let list_res = client.request("resources/list", json!({}));
    let resources = list_res
        .pointer("/result/resources")
        .and_then(Value::as_array)
        .expect("resources array");
    assert_eq!(resources.len(), 1);
    assert_eq!(
        resources[0].get("uri").and_then(Value::as_str),
        Some(UI_RESOURCE_URI)
    );
    assert_eq!(
        resources[0].get("mimeType").and_then(Value::as_str),
        Some(UI_RESOURCE_MIME_TYPE)
    );

    let read_res = client.request("resources/read", json!({ "uri": UI_RESOURCE_URI }));
    let content = &read_res
        .pointer("/result/contents/0")
        .expect("resource content");
    assert_eq!(
        content.get("uri").and_then(Value::as_str),
        Some(UI_RESOURCE_URI)
    );
    assert_eq!(
        content.get("mimeType").and_then(Value::as_str),
        Some(UI_RESOURCE_MIME_TYPE)
    );
    assert_eq!(
        content
            .pointer("/_meta/ui/prefersBorder")
            .and_then(Value::as_bool),
        Some(true)
    );
    let html = content
        .get("text")
        .and_then(Value::as_str)
        .expect("html text");
    assert!(html.starts_with("<!DOCTYPE html>"));
    for token in [
        "ui/initialize",
        "ui/notifications/initialized",
        "ui/notifications/tool-result",
        "ui/notifications/size-changed",
        "bead_show",
        "bead_ready",
    ] {
        assert!(html.contains(token), "html missing {token}");
    }
    assert!(!html.contains("http://"));
    assert!(!html.contains("https://"));

    let missing = client.request(
        "resources/read",
        json!({ "uri": "ui://beadroll/nope.html" }),
    );
    assert_eq!(
        missing.pointer("/error/code").and_then(Value::as_i64),
        Some(-32002)
    );

    let templates = client.request("resources/templates/list", json!({}));
    assert_eq!(
        templates.pointer("/result/resourceTemplates"),
        Some(&json!([]))
    );

    assert_eq!(client.close(), Some(0));
}

#[test]
fn malformed_json_and_invalid_tool_params_return_jsonrpc_errors() {
    let dir = tempdir().expect("tempdir");
    let mut client = McpClient::spawn(dir.path(), &[], &[]);
    client.initialize();

    client.send_raw("{not valid json");
    let parse_err = client.read_json();
    assert_eq!(
        parse_err.pointer("/error/code").and_then(Value::as_i64),
        Some(-32700)
    );

    client.send_raw("[]");
    let invalid_req = client.read_json();
    assert_eq!(
        invalid_req.pointer("/error/code").and_then(Value::as_i64),
        Some(-32600)
    );

    let unknown_tool = client.request(
        "tools/call",
        json!({ "name": "bead_claim", "arguments": { "id": "ex-1" } }),
    );
    assert_eq!(
        unknown_tool.pointer("/error/code").and_then(Value::as_i64),
        Some(-32602)
    );

    let bad_limit = client.request(
        "tools/call",
        json!({ "name": "bead_ready", "arguments": { "limit": 26 } }),
    );
    assert_eq!(
        bad_limit.pointer("/error/code").and_then(Value::as_i64),
        Some(-32602)
    );

    let extra_arg = client.request(
        "tools/call",
        json!({ "name": "bead_ready", "arguments": { "repo": "/tmp" } }),
    );
    assert_eq!(
        extra_arg.pointer("/error/code").and_then(Value::as_i64),
        Some(-32602)
    );

    let empty_show = client.request(
        "tools/call",
        json!({ "name": "bead_show", "arguments": {} }),
    );
    assert_eq!(
        empty_show.pointer("/error/code").and_then(Value::as_i64),
        Some(-32602)
    );

    assert_eq!(client.close(), Some(0));
}
