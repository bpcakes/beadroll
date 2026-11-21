//! MCP JSON-RPC 2.0 protocol loop and request dispatcher over stdio.

use std::io::{self, BufRead, Write};

use serde_json::{Map, Value, json};

use crate::bead::BeadCli;
use crate::cli::VERSION;
use crate::tools::{self, DispatchOutcome};
use crate::{Config, UI_HTML, UI_RESOURCE_MIME_TYPE, UI_RESOURCE_URI};

/// Protocol versions accepted from MCP clients during `initialize` negotiation.
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] =
    ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// Latest protocol version returned when a client asks for an unfamiliar version.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-11-25";

/// JSON-RPC 2.0 error codes.
const ERR_PARSE: i64 = -32700;
const ERR_INVALID_REQUEST: i64 = -32600;
const ERR_METHOD_NOT_FOUND: i64 = -32601;
const ERR_INVALID_PARAMS: i64 = -32602;
const ERR_RESOURCE_NOT_FOUND: i64 = -32002;

/// Stateful MCP server bound to one checkout [`Config`].
pub struct Server {
    cli: BeadCli,
}

impl Server {
    /// Creates an MCP server bound to `config`.
    pub fn new(config: Config) -> Self {
        Self {
            cli: BeadCli::new(config),
        }
    }

    /// Serves newline-delimited JSON-RPC 2.0 messages from `reader` onto `writer` until EOF.
    pub fn serve(&mut self, mut reader: impl BufRead, mut writer: impl Write) -> io::Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                return Ok(());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(response) = self.handle_line(trimmed) {
                let encoded = serde_json::to_string(&response)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                writer.write_all(encoded.as_bytes())?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
        }
    }

    /// Processes a single JSON-RPC line and returns the response object, or `None` for
    /// notifications.
    pub fn handle_line(&mut self, line: &str) -> Option<Value> {
        let parsed: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                return Some(error_response(
                    Value::Null,
                    ERR_PARSE,
                    format!("parse error: {error}"),
                ));
            }
        };
        let Some(obj) = parsed.as_object() else {
            return Some(error_response(
                Value::Null,
                ERR_INVALID_REQUEST,
                "request must be a JSON object",
            ));
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // Responses (result/error without method) need no reply; malformed requests with an
            // id receive Invalid Request.
            if obj.contains_key("result") || obj.contains_key("error") {
                return None;
            }
            return Some(error_response(
                id.unwrap_or(Value::Null),
                ERR_INVALID_REQUEST,
                "missing string `method`",
            ));
        };
        // Notifications omit `id` and receive no JSON-RPC response.
        let id = id?;
        let params = obj.get("params");
        Some(self.dispatch_request(id, method, params))
    }

    fn dispatch_request(&self, id: Value, method: &str, params: Option<&Value>) -> Value {
        match method {
            "initialize" => handle_initialize(id, params),
            "ping" => ok_response(id, json!({})),
            "tools/list" => ok_response(id, json!({ "tools": tools::definitions() })),
            "tools/call" => self.handle_tools_call(id, params),
            "resources/list" => ok_response(id, json!({ "resources": [ui_resource_entry()] })),
            "resources/templates/list" => ok_response(id, json!({ "resourceTemplates": [] })),
            "resources/read" => handle_resources_read(id, params),
            _ => error_response(
                id,
                ERR_METHOD_NOT_FOUND,
                format!("method not found: {method}"),
            ),
        }
    }

    fn handle_tools_call(&self, id: Value, params: Option<&Value>) -> Value {
        let Some(obj) = params.and_then(Value::as_object) else {
            return error_response(
                id,
                ERR_INVALID_PARAMS,
                "tools/call params must be an object",
            );
        };
        let Some(name) = obj.get("name").and_then(Value::as_str) else {
            return error_response(id, ERR_INVALID_PARAMS, "tools/call requires string `name`");
        };
        let empty = Map::new();
        let args = match obj.get("arguments") {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(map)) => map,
            Some(_) => {
                return error_response(
                    id,
                    ERR_INVALID_PARAMS,
                    "tools/call `arguments` must be an object",
                );
            }
        };
        match tools::call(&self.cli, name, args) {
            DispatchOutcome::Result(result) => ok_response(id, result),
            DispatchOutcome::InvalidParams(message) => {
                error_response(id, ERR_INVALID_PARAMS, message)
            }
        }
    }
}

fn handle_initialize(id: Value, params: Option<&Value>) -> Value {
    let requested = params
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("protocolVersion"))
        .and_then(Value::as_str);
    let negotiated = requested
        .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    ok_response(
        id,
        json!({
            "protocolVersion": negotiated,
            "capabilities": {
                "tools": {},
                "resources": {},
                "extensions": {
                    "io.modelcontextprotocol/ui": {
                        "mimeTypes": [UI_RESOURCE_MIME_TYPE]
                    }
                }
            },
            "serverInfo": {
                "name": "beadroll-mcp",
                "title": "Beadroll",
                "version": VERSION
            },
            "instructions": "Read-only beadroll issue browser bound to one checkout. Use bead_ready to list ready issues and bead_show to inspect an issue; writes and `bead sync` stay CLI commands."
        }),
    )
}

fn handle_resources_read(id: Value, params: Option<&Value>) -> Value {
    let Some(uri) = params
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("uri"))
        .and_then(Value::as_str)
    else {
        return error_response(
            id,
            ERR_INVALID_PARAMS,
            "resources/read requires string `uri`",
        );
    };
    if uri != UI_RESOURCE_URI {
        return error_response(
            id,
            ERR_RESOURCE_NOT_FOUND,
            format!("resource not found: {uri}"),
        );
    }
    ok_response(
        id,
        json!({
            "contents": [
                {
                    "uri": UI_RESOURCE_URI,
                    "mimeType": UI_RESOURCE_MIME_TYPE,
                    "text": UI_HTML,
                    "_meta": {
                        "ui": {
                            "prefersBorder": true
                        }
                    }
                }
            ]
        }),
    )
}

fn ui_resource_entry() -> Value {
    json!({
        "uri": UI_RESOURCE_URI,
        "name": "beadroll_issues",
        "title": "Beadroll Issues",
        "description": "Interactive read-only browser for ready beadroll issues and issue details.",
        "mimeType": UI_RESOURCE_MIME_TYPE,
        "_meta": {
            "ui": {
                "prefersBorder": true
            }
        }
    })
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn error_response(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        }
    })
}
