//! MCP JSON-RPC 2.0 protocol loop and request dispatcher over stdio.

use std::io::{self, BufRead, Write};

use serde_json::{Map, Value, json};

use crate::bead::BeadCli;
use crate::cli::VERSION;
use crate::tools::{self, DispatchOutcome};
use crate::{Config, MAX_FRAME_BYTES, UI_HTML, UI_RESOURCE_MIME_TYPE, UI_RESOURCE_URI};

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

/// Outcome of reading one newline-delimited byte frame from stdin.
enum Frame {
    Eof,
    Bytes(Vec<u8>),
    TooLarge,
}

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
    ///
    /// Invalid UTF-8 frames and frames larger than [`MAX_FRAME_BYTES`] receive a JSON-RPC error
    /// response without terminating the session.
    pub fn serve(&mut self, mut reader: impl BufRead, mut writer: impl Write) -> io::Result<()> {
        loop {
            let response = match read_bounded_frame(&mut reader, MAX_FRAME_BYTES)? {
                Frame::Eof => return Ok(()),
                Frame::TooLarge => Some(error_response(
                    Value::Null,
                    ERR_INVALID_REQUEST,
                    format!("message exceeds {MAX_FRAME_BYTES}-byte limit"),
                )),
                Frame::Bytes(raw) => self.handle_bytes(&raw),
            };
            if let Some(response) = response {
                let encoded = serde_json::to_string(&response)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                writer.write_all(encoded.as_bytes())?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
        }
    }

    /// Processes a single raw byte line, returning a parse error if it is not valid UTF-8.
    fn handle_bytes(&mut self, raw: &[u8]) -> Option<Value> {
        let Ok(text) = std::str::from_utf8(raw) else {
            return Some(error_response(
                Value::Null,
                ERR_PARSE,
                "parse error: message is not valid UTF-8",
            ));
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        self.handle_line(trimmed)
    }

    /// Processes a single JSON-RPC line and returns the response object, or `None` for valid
    /// notifications and responses.
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
        if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error_response(
                Value::Null,
                ERR_INVALID_REQUEST,
                "missing or unsupported `jsonrpc` version (expected \"2.0\")",
            ));
        }
        let Some(method_val) = obj.get("method") else {
            // Valid JSON-RPC responses carry `result` or `error` without `method` and receive no
            // reply; anything else is an invalid request.
            if obj.contains_key("result") || obj.contains_key("error") {
                return None;
            }
            return Some(error_response(
                Value::Null,
                ERR_INVALID_REQUEST,
                "missing string `method`",
            ));
        };
        let Some(method) = method_val.as_str().filter(|name| !name.is_empty()) else {
            return Some(error_response(
                Value::Null,
                ERR_INVALID_REQUEST,
                "`method` must be a non-empty string",
            ));
        };
        // Notifications omit `id` entirely and must never receive a response.
        let id_val = obj.get("id")?;
        if !is_valid_request_id(id_val) {
            return Some(error_response(
                Value::Null,
                ERR_INVALID_REQUEST,
                "request `id` must be a string or integer",
            ));
        }
        let id = id_val.clone();
        let params = match validate_params(obj.get("params")) {
            Ok(params) => params,
            Err((code, message)) => return Some(error_response(id, code, message)),
        };
        Some(self.dispatch_request(id, method, params))
    }

    fn dispatch_request(
        &self,
        id: Value,
        method: &str,
        params: Option<&Map<String, Value>>,
    ) -> Value {
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

    fn handle_tools_call(&self, id: Value, params: Option<&Map<String, Value>>) -> Value {
        let Some(obj) = params else {
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
            DispatchOutcome::UnknownTool(message) => {
                error_response(id, ERR_INVALID_PARAMS, message)
            }
        }
    }
}

/// Reads bytes up to `\n` (or EOF) without allocating beyond `limit` bytes. When a line exceeds
/// `limit`, drains the rest of the line so the next newline begins a fresh frame cleanly.
fn read_bounded_frame(reader: &mut impl BufRead, limit: usize) -> io::Result<Frame> {
    let mut buf = Vec::new();
    let mut overflowed = false;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(if overflowed {
                Frame::TooLarge
            } else if buf.is_empty() {
                Frame::Eof
            } else {
                Frame::Bytes(buf)
            });
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(chunk.len(), |pos| pos + 1);
        if !overflowed {
            if buf.len().saturating_add(take) > limit {
                overflowed = true;
                buf.clear();
            } else if let Some(slice) = chunk.get(..take) {
                buf.extend_from_slice(slice);
            }
        }
        reader.consume(take);
        if newline.is_some() {
            return Ok(if overflowed {
                Frame::TooLarge
            } else {
                Frame::Bytes(buf)
            });
        }
    }
}

/// MCP base protocol restricts request IDs to strings or integers (never null, bool, float,
/// array, or object).
fn is_valid_request_id(value: &Value) -> bool {
    match value {
        Value::String(_) => true,
        Value::Number(num) => num.is_i64() || num.is_u64(),
        _ => false,
    }
}

/// Validates optional `params`: omitted means `Ok(None)`, an object means `Ok(Some(map))`, an
/// array returns `-32602` (MCP methods require named object params), and primitive/null values
/// return `-32600` (invalid JSON-RPC 2.0 envelope).
fn validate_params(
    params: Option<&Value>,
) -> Result<Option<&Map<String, Value>>, (i64, &'static str)> {
    match params {
        None => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map)),
        Some(Value::Array(_)) => Err((ERR_INVALID_PARAMS, "`params` must be an object")),
        Some(_) => Err((ERR_INVALID_REQUEST, "`params` must be a JSON object")),
    }
}

fn handle_initialize(id: Value, params: Option<&Map<String, Value>>) -> Value {
    let requested = params
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

fn handle_resources_read(id: Value, params: Option<&Map<String, Value>>) -> Value {
    let Some(uri) = params
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
