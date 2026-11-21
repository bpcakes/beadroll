//! Read-only MCP tool declarations (`bead_ready`, `bead_show`), strict argument validation, and
//! dispatch.

use serde_json::{Map, Value, json};

use crate::bead::{AppError, BeadCli, ShowTarget};
use crate::format::{format_detail, format_ready};
use crate::{MAX_READY_LIMIT, UI_RESOURCE_URI};

/// Tool name for listing ready issues.
pub(crate) const TOOL_READY: &str = "bead_ready";

/// Tool name for showing one issue in full.
pub(crate) const TOOL_SHOW: &str = "bead_show";

/// Result of invoking a tool: either an MCP `CallToolResult` (including tool execution and
/// argument-validation errors marked `isError: true`) or a JSON-RPC `-32602` unknown-tool error.
pub(crate) enum DispatchOutcome {
    /// Tool executed or returned a structured tool error (`isError: true`).
    Result(Value),
    /// Unknown tool name (JSON-RPC `-32602`).
    UnknownTool(String),
}

/// Returns the `tools` array advertised by `tools/list`.
///
/// Both tools declare `annotations.readOnlyHint = true` so hosts such as T3 Code execute clicks
/// and refreshes inside the MCP App without prompting for approval, and declare
/// `_meta.ui.resourceUri` unconditionally because some hosts (including Claude Code) do not
/// advertise `io.modelcontextprotocol/ui` in `initialize` capabilities.
pub(crate) fn definitions() -> Value {
    json!([
        {
            "name": TOOL_READY,
            "title": "Ready issues",
            "description": "List up to 25 ready issues from the bound beadroll checkout in beadroll's order, with workspace identity and last sync time. Performs local reads only and never syncs or modifies the tracker.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_READY_LIMIT,
                        "description": "Maximum number of ready issues to return (default 25, max 25)."
                    }
                },
                "additionalProperties": false
            },
            "annotations": {
                "title": "Ready issues",
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            },
            "_meta": {
                "ui": {
                    "resourceUri": UI_RESOURCE_URI,
                    "visibility": ["model", "app"]
                }
            }
        },
        {
            "name": TOOL_SHOW,
            "title": "Show issue",
            "description": "Show full details for one issue in the bound beadroll checkout (description, acceptance criteria, status, claim owner, dependencies, and comments). Pass uid and workspace when reopening an issue from a previous result, or id for an initial lookup by key. Performs local reads only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Issue display key (for example `roll-ui9r`) or stable uid for an initial lookup."
                    },
                    "uid": {
                        "type": "string",
                        "description": "Stable issue uid from a prior bead_ready or bead_show result. Preferred over id when both are supplied."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Expected workspace ID from a prior result; refused if it does not match this server's bound checkout."
                    }
                },
                "additionalProperties": false
            },
            "annotations": {
                "title": "Show issue",
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": false
            },
            "_meta": {
                "ui": {
                    "resourceUri": UI_RESOURCE_URI,
                    "visibility": ["model", "app"]
                }
            }
        }
    ])
}

/// Validates arguments and runs `name` against `cli`.
///
/// Per MCP 2025-11-25, invalid arguments to a known tool return a `CallToolResult` with
/// `isError: true` (`INVALID_ARGUMENT`) so models and MCP Apps receive actionable feedback; an
/// unknown tool name returns JSON-RPC `-32602`.
pub(crate) fn call(cli: &BeadCli, name: &str, args: &Map<String, Value>) -> DispatchOutcome {
    match name {
        TOOL_READY => DispatchOutcome::Result(call_ready(cli, args)),
        TOOL_SHOW => DispatchOutcome::Result(call_show(cli, args)),
        _ => DispatchOutcome::UnknownTool(format!("unknown tool `{name}`")),
    }
}

fn call_ready(cli: &BeadCli, args: &Map<String, Value>) -> Value {
    if let Some(unknown) = unknown_key(args, &["limit"]) {
        return invalid_arg(format!("bead_ready does not accept argument `{unknown}`"));
    }
    let limit = match args.get("limit") {
        None | Some(Value::Null) => MAX_READY_LIMIT,
        Some(value) => match parse_limit(value) {
            Ok(limit) => limit,
            Err(message) => return invalid_arg(message),
        },
    };
    match cli.ready(limit) {
        Ok(view) => {
            let text = format_ready(&view);
            let structured = view.to_json();
            ok_result(text, structured)
        }
        Err(error) => error_result(&error),
    }
}

fn call_show(cli: &BeadCli, args: &Map<String, Value>) -> Value {
    if let Some(unknown) = unknown_key(args, &["id", "uid", "workspace"]) {
        return invalid_arg(format!("bead_show does not accept argument `{unknown}`"));
    }
    let uid = match optional_trimmed_string(args, "uid") {
        Ok(value) => value,
        Err(message) => return invalid_arg(message),
    };
    let id = match optional_trimmed_string(args, "id") {
        Ok(value) => value,
        Err(message) => return invalid_arg(message),
    };
    let workspace = match optional_trimmed_string(args, "workspace") {
        Ok(value) => value,
        Err(message) => return invalid_arg(message),
    };
    // Prefer the explicit stable `uid` whenever both `uid` and `id` are supplied, and retain the
    // distinction so `BeadCli::show` refuses any key-colliding response with a different UID.
    let target = match (uid, id) {
        (Some(uid), _) => ShowTarget::Uid(uid),
        (None, Some(id)) => ShowTarget::Id(id),
        (None, None) => {
            return invalid_arg("bead_show requires a non-empty `uid` or `id`");
        }
    };
    match cli.show(&target, workspace.as_deref()) {
        Ok(view) => {
            let text = format_detail(&view);
            let structured = view.to_json();
            ok_result(text, structured)
        }
        Err(error) => error_result(&error),
    }
}

fn ok_result(text: String, structured: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
    })
}

fn invalid_arg(message: impl Into<String>) -> Value {
    error_result(&AppError::invalid_argument(message))
}

fn error_result(error: &AppError) -> Value {
    json!({
        "isError": true,
        "content": [{ "type": "text", "text": format!("{}: {error}", error.code()) }],
        "structuredContent": error.to_structured_json(),
    })
}

fn unknown_key<'a>(args: &'a Map<String, Value>, allowed: &[&str]) -> Option<&'a str> {
    args.keys()
        .map(String::as_str)
        .find(|key| !allowed.contains(key))
}

fn parse_limit(value: &Value) -> Result<usize, String> {
    let Some(number) = value.as_u64() else {
        return Err(format!(
            "bead_ready `limit` must be an integer between 1 and {MAX_READY_LIMIT}"
        ));
    };
    let Ok(limit) = usize::try_from(number) else {
        return Err(format!(
            "bead_ready `limit` must be between 1 and {MAX_READY_LIMIT}"
        ));
    };
    if (1..=MAX_READY_LIMIT).contains(&limit) {
        Ok(limit)
    } else {
        Err(format!(
            "bead_ready `limit` must be between 1 and {MAX_READY_LIMIT}"
        ))
    }
}

fn optional_trimmed_string(args: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Err(format!("bead_show `{key}` must not be empty"))
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(_) => Err(format!("bead_show `{key}` must be a string")),
    }
}
