//! MCP JSON-RPC 2.0 protocol loop and strict request envelope dispatcher over stdio.

use std::fmt;
use std::io::{self, BufRead, Write};

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
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

/// Top-level JSON-RPC 2.0 envelope decoded with duplicate-key rejection.
struct Envelope {
    jsonrpc: Option<Value>,
    id: Option<Value>,
    method: Option<Value>,
    params: Option<Value>,
    has_result: bool,
    error: Option<Value>,
    duplicate_key: Option<String>,
}

enum ParsedEnvelope {
    Object(Envelope),
    NonObject,
}

impl<'de> Deserialize<'de> for ParsedEnvelope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EnvelopeVisitor;

        impl<'de> Visitor<'de> for EnvelopeVisitor {
            type Value = ParsedEnvelope;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON value")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut seen = std::collections::BTreeSet::new();
                let mut env = Envelope {
                    jsonrpc: None,
                    id: None,
                    method: None,
                    params: None,
                    has_result: false,
                    error: None,
                    duplicate_key: None,
                };
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    if !seen.insert(key.clone()) && env.duplicate_key.is_none() {
                        env.duplicate_key = Some(key.clone());
                    }
                    match key.as_str() {
                        "jsonrpc" => env.jsonrpc = Some(value),
                        "id" => env.id = Some(value),
                        "method" => env.method = Some(value),
                        "params" => env.params = Some(value),
                        "result" => env.has_result = true,
                        "error" => env.error = Some(value),
                        _ => {}
                    }
                }
                Ok(ParsedEnvelope::Object(env))
            }

            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(ParsedEnvelope::NonObject)
            }

            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(ParsedEnvelope::NonObject)
            }

            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(ParsedEnvelope::NonObject)
            }

            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(ParsedEnvelope::NonObject)
            }

            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(ParsedEnvelope::NonObject)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(ParsedEnvelope::NonObject)
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                while seq.next_element::<Value>()?.is_some() {}
                Ok(ParsedEnvelope::NonObject)
            }
        }

        deserializer.deserialize_any(EnvelopeVisitor)
    }
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
    /// Invalid UTF-8 frames and frames whose payload exceeds [`MAX_FRAME_BYTES`] receive a
    /// JSON-RPC error response without terminating the session.
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
    /// notifications and valid client responses.
    pub fn handle_line(&mut self, line: &str) -> Option<Value> {
        let parsed: ParsedEnvelope = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                return Some(error_response(
                    Value::Null,
                    ERR_PARSE,
                    format!("parse error: {error}"),
                ));
            }
        };
        let ParsedEnvelope::Object(env) = parsed else {
            return Some(error_response(
                Value::Null,
                ERR_INVALID_REQUEST,
                "request must be a JSON object",
            ));
        };
        self.handle_envelope(env)
    }

    fn handle_envelope(&self, env: Envelope) -> Option<Value> {
        if let Some(dup) = env.duplicate_key {
            return Some(error_response(
                Value::Null,
                ERR_INVALID_REQUEST,
                format!("duplicate key `{dup}` in JSON-RPC envelope"),
            ));
        }
        if env.jsonrpc.as_ref().and_then(Value::as_str) != Some("2.0") {
            return Some(error_response(
                Value::Null,
                ERR_INVALID_REQUEST,
                "missing or unsupported `jsonrpc` version (expected \"2.0\")",
            ));
        }
        let Some(method_val) = env.method else {
            return validate_client_response(&env);
        };
        if env.has_result || env.error.is_some() {
            return Some(error_response(
                Value::Null,
                ERR_INVALID_REQUEST,
                "request must not contain `result` or `error`",
            ));
        }
        let Some(method) = method_val.as_str().filter(|name| !name.is_empty()) else {
            return Some(error_response(
                Value::Null,
                ERR_INVALID_REQUEST,
                "`method` must be a non-empty string",
            ));
        };
        // Notifications omit `id`: valid notifications stay silent; invalid non-object params are
        // rejected as Invalid Request rather than silently ignored.
        let Some(id) = env.id else {
            return match validate_params(env.params.as_ref()) {
                Ok(_) => None,
                Err((code, message)) => Some(error_response(Value::Null, code, message)),
            };
        };
        if !is_valid_request_id(&id) {
            return Some(error_response(
                Value::Null,
                ERR_INVALID_REQUEST,
                "request `id` must be a string or integer",
            ));
        }
        let params = match validate_params(env.params.as_ref()) {
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

/// Validates a methodless JSON-RPC 2.0 object: a valid response (`result` XOR valid error object
/// with integer `code` and string `message`, valid `id` or `null` for error responses, no
/// `params`) is silently consumed; anything else returns -32600.
fn validate_client_response(env: &Envelope) -> Option<Value> {
    let has_error = env.error.is_some();
    let valid_outcome = env.has_result ^ has_error;
    let valid_error_shape = env.error.as_ref().is_none_or(is_valid_error_object);
    let valid_id = match env.id.as_ref() {
        Some(Value::Null) => has_error,
        Some(id) => is_valid_request_id(id),
        None => false,
    };
    if valid_outcome && valid_error_shape && valid_id && env.params.is_none() {
        None
    } else {
        Some(error_response(
            Value::Null,
            ERR_INVALID_REQUEST,
            "missing string `method` or malformed JSON-RPC response",
        ))
    }
}

fn is_valid_error_object(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    let valid_code = obj
        .get("code")
        .and_then(Value::as_number)
        .is_some_and(|n| n.is_i64() || n.is_u64());
    let valid_message = obj.get("message").is_some_and(Value::is_string);
    valid_code && valid_message
}

/// Reads bytes up to `\n` (or EOF) without allocating beyond `limit` payload bytes (the trailing
/// `\n` or `\r\n` delimiter is not counted toward `limit`). When a line exceeds `limit`, drains
/// through the next `\n` so the following line starts cleanly.
fn read_bounded_frame(reader: &mut impl BufRead, limit: usize) -> io::Result<Frame> {
    let mut buf = Vec::new();
    let mut overflowed = false;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(if overflowed || buf.len() > limit {
                Frame::TooLarge
            } else if buf.is_empty() {
                Frame::Eof
            } else {
                Frame::Bytes(buf)
            });
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(chunk.len(), |pos| pos + 1);
        let payload_len = newline.unwrap_or(chunk.len());
        if !overflowed && let Some(slice) = chunk.get(..payload_len) {
            let trimmed_len = if newline.is_some() && slice.ends_with(b"\r") {
                slice.len() - 1
            } else {
                slice.len()
            };
            if newline == Some(0) && buf.ends_with(b"\r") {
                buf.pop();
            }
            // Allow one extra byte before `\n` in case `\r\n` splits across buffer fills.
            let max_buffered = if newline.is_some() {
                limit
            } else {
                limit.saturating_add(1)
            };
            if buf.len().saturating_add(trimmed_len) > max_buffered {
                overflowed = true;
                buf.clear();
            } else if let Some(payload) = slice.get(..trimmed_len) {
                buf.extend_from_slice(payload);
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(if overflowed || buf.len() > limit {
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
