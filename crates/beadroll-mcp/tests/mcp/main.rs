//! Integration tests for `beadroll-mcp`: MCP JSON-RPC 2.0 protocol, read-only browsing,
//! checkout/workspace binding, stable-UID reopening across key renames, UI contract invariants,
//! and real `bead` runs.
#![cfg(test)]

mod binding;
mod browsing;
mod protocol;
mod real_cli;
mod stable_id;
mod ui_contract;

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use beadroll_test_support::executable;
use serde_json::{Value, json};

/// Path to the `beadroll-mcp` executable built by Cargo for this test binary.
pub(crate) fn mcp_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_beadroll-mcp"))
}

/// Live `beadroll-mcp` child process spoken to over newline-delimited JSON-RPC 2.0 on stdio.
pub(crate) struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpClient {
    /// Spawns `beadroll-mcp` with `args`, `cwd`, and `env_overrides`.
    pub(crate) fn spawn(cwd: &Path, args: &[&str], env_overrides: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(mcp_bin());
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env_overrides {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().expect("spawn beadroll-mcp");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            next_id: 1,
        }
    }

    /// Sends one raw UTF-8 line to stdin without waiting for a response.
    pub(crate) fn send_raw(&mut self, line: &str) {
        self.send_bytes(line.as_bytes());
    }

    /// Sends raw bytes followed by `\n` to stdin (supports invalid UTF-8 and oversized frames).
    pub(crate) fn send_bytes(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().expect("stdin still open");
        stdin.write_all(bytes).expect("write request bytes");
        stdin.write_all(b"\n").expect("write newline");
        stdin.flush().expect("flush stdin");
    }

    /// Reads one JSON-RPC response line from stdout.
    pub(crate) fn read_json(&mut self) -> Value {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).expect("read stdout line");
        assert!(n > 0, "beadroll-mcp closed stdout unexpectedly");
        serde_json::from_str(line.trim())
            .unwrap_or_else(|error| panic!("invalid JSON from server ({error}): {line:?}"))
    }

    /// Sends a JSON-RPC request with an auto-incremented numeric id and returns the response.
    pub(crate) fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.send_raw(&serde_json::to_string(&msg).expect("encode request"));
        let resp = self.read_json();
        assert_eq!(resp.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
        assert_eq!(resp.get("id").and_then(Value::as_i64), Some(id));
        resp
    }

    /// Sends a JSON-RPC notification (no id, no response expected).
    pub(crate) fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.send_raw(&serde_json::to_string(&msg).expect("encode notification"));
    }

    /// Runs the standard MCP `initialize` + `notifications/initialized` handshake.
    pub(crate) fn initialize(&mut self) -> Value {
        let resp = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1.0.0" }
            }),
        );
        self.notify("notifications/initialized", json!({}));
        resp
    }

    /// Calls `tools/call` for `name` with `arguments` and returns the JSON-RPC `result` object.
    pub(crate) fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        let resp = self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
            }),
        );
        resp.get("result")
            .cloned()
            .unwrap_or_else(|| panic!("expected result, got: {resp}"))
    }

    /// Closes stdin and returns the server's exit status code.
    pub(crate) fn close(mut self) -> Option<i32> {
        self.stdin.take();
        let status = self.child.wait().expect("wait for beadroll-mcp");
        status.code()
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Writes an executable fake `bead` script at `dir/name` backed by Python 3 and returns its path.
pub(crate) fn write_fake_bead(dir: &Path, name: &str, python_body: &str) -> PathBuf {
    let script = dir.join(name);
    let content = format!("#!/usr/bin/env python3\n{python_body}\n");
    executable(&script, content);
    script
}

/// Writes a fake `bead` script that dispatches `info`, `ready`, and `show` from a JSON fixture
/// file at `state_file` (reread on every invocation so tests can mutate it to verify Refresh and
/// key renames) and logs every call's cwd, argv, `GIT_*` env, and `BEAD_*` env into `log_file`.
pub(crate) fn write_stateful_fake_bead(
    dir: &Path,
    name: &str,
    state_file: &Path,
    log_file: &Path,
) -> PathBuf {
    let state_repr = serde_json::to_string(&state_file.display().to_string()).expect("path json");
    let log_repr = serde_json::to_string(&log_file.display().to_string()).expect("log json");
    let body = format!(
        r#"import json, os, sys

STATE = {state_repr}
LOG = {log_repr}

with open(LOG, "a") as f:
    git_env = {{k: v for k, v in os.environ.items() if k.startswith("GIT_")}}
    bead_env = {{k: v for k, v in os.environ.items() if k.startswith("BEAD_")}}
    f.write(json.dumps({{
        "cwd": os.getcwd(),
        "argv": sys.argv[1:],
        "git_env": git_env,
        "bead_env": bead_env,
    }}) + "\n")

with open(STATE) as f:
    data = json.load(f)

args = sys.argv[1:]
if not args:
    sys.exit(2)

cmd = args[0]
if cmd == "info":
    if "info_error" in data:
        sys.stderr.write(json.dumps({{"error": data["info_error"]}}) + "\n")
        sys.exit(1)
    print(json.dumps(data["info"]))
    sys.exit(0)

if cmd == "ready":
    if "ready_error" in data:
        sys.stderr.write(json.dumps({{"error": data["ready_error"]}}) + "\n")
        sys.exit(1)
    limit = None
    if "--limit" in args:
        idx = args.index("--limit")
        limit = int(args[idx + 1])
    issues = data.get("ready", [])
    if limit is not None and limit > 0:
        issues = issues[:limit]
    print(json.dumps(issues))
    sys.exit(0)

if cmd == "show":
    if "show_error" in data:
        sys.stderr.write(json.dumps({{"error": data["show_error"]}}) + "\n")
        sys.exit(1)
    target = args[-1]
    details = data.get("details", {{}})
    # Keys match first, then uids (matching beadroll's commands::resolve precedence).
    for item in details.values():
        if item.get("id") == target:
            print(json.dumps([item]))
            sys.exit(0)
    for item in details.values():
        if item.get("uid") == target:
            print(json.dumps([item]))
            sys.exit(0)
    sys.stderr.write(json.dumps({{"error": {{"code": "ISSUE_NOT_FOUND", "message": f"issue {{target}} not found"}}}}) + "\n")
    sys.exit(1)

sys.stderr.write(json.dumps({{"error": {{"code": "FORBIDDEN_WRITE", "message": f"unexpected command {{cmd}}"}}}}) + "\n")
sys.exit(1)
"#
    );
    write_fake_bead(dir, name, &body)
}
