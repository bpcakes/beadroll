//! MCP JSON-RPC 2.0 handshake, envelope validation, bounded frame reading, tool/resource
//! declarations, UI HTML invariants, and error framing.

use beadroll_mcp::{MAX_FRAME_BYTES, UI_RESOURCE_MIME_TYPE, UI_RESOURCE_URI};
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

    // Notifications and valid client responses (result or error) produce no reply.
    client.notify("notifications/initialized", json!({}));
    client.notify("notifications/cancelled", json!({ "requestId": 999 }));
    client.send_raw(r#"{"jsonrpc":"2.0","id":"ignored-response","result":{}}"#);
    client.send_raw(r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32000,"message":"ignored"}}"#);
    client.send_raw(r#"{"jsonrpc":"2.0","id":"str-id-1","method":"ping"}"#);
    let ping = client.read_json();
    assert_eq!(ping.get("id"), Some(&json!("str-id-1")));
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
fn malformed_envelopes_invalid_utf8_and_oversized_frames_do_not_kill_session() {
    let dir = tempdir().expect("tempdir");
    let mut client = McpClient::spawn(dir.path(), &[], &[]);
    client.initialize();

    let bad_envelopes = [
        ("{not valid json", -32700, Value::Null),
        ("[]", -32600, Value::Null),
        (
            r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#,
            -32600,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"1.0","jsonrpc":"2.0","id":1,"method":"ping"}"#,
            -32600,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","result":{}}"#,
            -32600,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"2.0","result":{},"error":{"code":-1,"message":"x"}}"#,
            -32600,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"error":null}"#,
            -32600,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":"bad","message":"x"}}"#,
            -32600,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000}}"#,
            -32600,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"2.0","method":"notifications/progress","params":3}"#,
            -32600,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"2.0","id":{},"method":"ping"}"#,
            -32600,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#,
            -32600,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"2.0","id":true,"method":"ping"}"#,
            -32600,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/list","params":3}"#,
            -32600,
            json!(10),
        ),
        (
            r#"{"jsonrpc":"2.0","id":11,"method":"tools/list","params":[]}"#,
            -32602,
            json!(11),
        ),
    ];
    for (raw, expected_code, expected_id) in bad_envelopes {
        client.send_raw(raw);
        let err = client.read_json();
        assert_eq!(
            err.pointer("/error/code").and_then(Value::as_i64),
            Some(expected_code),
            "envelope {raw} got {err}"
        );
        assert_eq!(err.get("id"), Some(&expected_id), "envelope {raw}");
    }

    // Subsequent valid ping still succeeds.
    let ping = client.request("ping", json!({}));
    assert_eq!(ping.get("result"), Some(&json!({})));

    assert_eq!(client.close(), Some(0));
}

#[test]
fn invalid_utf8_exact_cap_crlf_and_oversized_frames_recover_cleanly() {
    let dir = tempdir().expect("tempdir");
    let mut client = McpClient::spawn(dir.path(), &[], &[]);
    client.initialize();

    // Invalid UTF-8 line returns parse error and session resumes cleanly.
    client.send_bytes(&[0xff, 0xfe, b'{', b'}']);
    let utf8_err = client.read_json();
    assert_eq!(
        utf8_err.pointer("/error/code").and_then(Value::as_i64),
        Some(-32700)
    );

    // Payload of exactly MAX_FRAME_BYTES followed by CRLF fits within the frame cap.
    let prefix = br#"{"jsonrpc":"2.0","id":"exact-max","method":"ping"}"#;
    let mut exact = Vec::with_capacity(MAX_FRAME_BYTES + 1);
    exact.extend_from_slice(prefix);
    exact.resize(MAX_FRAME_BYTES, b' ');
    exact.push(b'\r');
    client.send_bytes(&exact);
    let exact_reply = client.read_json();
    assert_eq!(exact_reply.get("id"), Some(&json!("exact-max")));
    assert_eq!(exact_reply.get("result"), Some(&json!({})));

    // Oversized line beyond MAX_FRAME_BYTES returns -32600 and session resumes cleanly.
    let oversized = vec![b'a'; MAX_FRAME_BYTES + 64];
    client.send_bytes(&oversized);
    let size_err = client.read_json();
    assert_eq!(
        size_err.pointer("/error/code").and_then(Value::as_i64),
        Some(-32600)
    );

    let ping = client.request("ping", json!({}));
    assert_eq!(ping.get("result"), Some(&json!({})));

    assert_eq!(client.close(), Some(0));
}

#[test]
fn unknown_tool_returns_rpc_error_while_invalid_tool_args_return_tool_errors() {
    let dir = tempdir().expect("tempdir");
    let mut client = McpClient::spawn(dir.path(), &[], &[]);
    client.initialize();

    let unknown_tool = client.request(
        "tools/call",
        json!({ "name": "bead_claim", "arguments": { "id": "ex-1" } }),
    );
    assert_eq!(
        unknown_tool.pointer("/error/code").and_then(Value::as_i64),
        Some(-32602)
    );

    for (name, args) in [
        ("bead_ready", json!({ "limit": 26 })),
        ("bead_ready", json!({ "repo": "/tmp" })),
        ("bead_show", json!({})),
        ("bead_show", json!({ "id": "   " })),
        ("bead_show", json!({ "id": "ex-1", "extra": true })),
    ] {
        let res = client.call_tool(name, args);
        assert_eq!(res.get("isError").and_then(Value::as_bool), Some(true));
        assert_eq!(
            res.pointer("/structuredContent/error/code")
                .and_then(Value::as_str),
            Some("INVALID_ARGUMENT")
        );
    }

    assert_eq!(client.close(), Some(0));
}
