//! Checkout session binding, isolation between two trackers sharing the same display key,
//! workspace mismatch refusal, and scrubbing of ambient git routing environment variables.

use serde_json::{Value, json};
use tempfile::tempdir;

use crate::{McpClient, write_stateful_fake_bead};

fn write_tracker_state(
    dir: &std::path::Path,
    ws_id: &str,
    name: &str,
    uid: &str,
    title: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let state_file = dir.join("state.json");
    let log_file = dir.join("bead.log");
    let issue = json!({
        "id": "dup-1",
        "uid": uid,
        "title": title,
        "description": format!("Belongs strictly to {name}."),
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "dependencies": [],
        "dependents": [],
        "comments": [],
        "blocked": false,
    });
    std::fs::write(
        &state_file,
        serde_json::to_string(&json!({
            "info": {
                "version": "0.2.0",
                "workspace": ws_id,
                "workspace_source": "origin url",
                "name": name,
                "prefix": "dup",
                "last_sync": "2026-11-21T12:00:00Z"
            },
            "ready": [&issue],
            "details": { "dup-1": issue }
        }))
        .expect("encode"),
    )
    .expect("write state");
    let fake_bead = write_stateful_fake_bead(dir, "fake-bead", &state_file, &log_file);
    (fake_bead, log_file)
}

#[test]
fn two_trackers_with_same_display_key_stay_isolated_and_refuse_wrong_workspace() {
    let dir = tempdir().expect("tempdir");
    let repo_a = dir.path().join("repo-a");
    let repo_b = dir.path().join("repo-b");
    std::fs::create_dir(&repo_a).expect("repo_a");
    std::fs::create_dir(&repo_b).expect("repo_b");

    let uid_a = "019a0000-0000-7000-8000-aaaaaaaaaaaa";
    let uid_b = "019a0000-0000-7000-8000-bbbbbbbbbbbb";
    let (bead_a, _log_a) =
        write_tracker_state(&repo_a, "ws-repo-a", "project-a", uid_a, "Issue in A");
    let (bead_b, log_b) =
        write_tracker_state(&repo_b, "ws-repo-b", "project-b", uid_b, "Issue in B");

    // Session A binds via startup cwd; Session B binds via explicit `--repo`.
    let mut client_a = McpClient::spawn(&repo_a, &["--bead", bead_a.to_str().expect("utf8")], &[]);
    client_a.initialize();

    let mut client_b = McpClient::spawn(
        dir.path(),
        &[
            "--repo",
            repo_b.to_str().expect("utf8"),
            "--bead",
            bead_b.to_str().expect("utf8"),
        ],
        &[],
    );
    client_b.initialize();

    let ready_a = client_a.call_tool("bead_ready", json!({}));
    let ready_b = client_b.call_tool("bead_ready", json!({}));
    assert_eq!(
        ready_a
            .pointer("/structuredContent/issues/0/title")
            .and_then(Value::as_str),
        Some("Issue in A")
    );
    assert_eq!(
        ready_b
            .pointer("/structuredContent/issues/0/title")
            .and_then(Value::as_str),
        Some("Issue in B")
    );

    // Looking up the same display key `dup-1` returns each bound checkout's own issue.
    let show_a = client_a.call_tool("bead_show", json!({ "id": "dup-1" }));
    let show_b = client_b.call_tool("bead_show", json!({ "id": "dup-1" }));
    assert_eq!(
        show_a
            .pointer("/structuredContent/issue/uid")
            .and_then(Value::as_str),
        Some(uid_a)
    );
    assert_eq!(
        show_b
            .pointer("/structuredContent/issue/uid")
            .and_then(Value::as_str),
        Some(uid_b)
    );

    // Passing Session A's workspace ID into Session B is refused before `bead show` runs.
    let before_calls = std::fs::read_to_string(&log_b)
        .expect("log_b")
        .lines()
        .count();
    let cross = client_b.call_tool(
        "bead_show",
        json!({ "uid": uid_a, "workspace": "ws-repo-a" }),
    );
    assert_eq!(cross.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        cross
            .pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("WORKSPACE_MISMATCH")
    );
    let after_log = std::fs::read_to_string(&log_b).expect("log_b");
    let new_lines: Vec<&str> = after_log.lines().skip(before_calls).collect();
    assert_eq!(
        new_lines.len(),
        1,
        "only `bead info` should run on mismatch"
    );
    let only_cmd: Value = serde_json::from_str(new_lines[0]).expect("json");
    assert_eq!(
        only_cmd.pointer("/argv/0").and_then(Value::as_str),
        Some("info")
    );

    assert_eq!(client_a.close(), Some(0));
    assert_eq!(client_b.close(), Some(0));
}

#[test]
fn ambient_git_routing_vars_are_scrubbed_from_bead_subprocesses() {
    let dir = tempdir().expect("tempdir");
    let repo = dir.path().join("checkout");
    std::fs::create_dir(&repo).expect("repo");
    let (fake_bead, log_file) = write_tracker_state(
        &repo,
        "ws-clean",
        "clean-proj",
        "019a0000-0000-7000-8000-cccccccccccc",
        "Scoped issue",
    );

    let mut client = McpClient::spawn(
        dir.path(),
        &[
            "--repo",
            repo.to_str().expect("utf8"),
            "--bead",
            fake_bead.to_str().expect("utf8"),
        ],
        &[
            ("GIT_DIR", "/elsewhere/.git"),
            ("GIT_WORK_TREE", "/elsewhere"),
            ("GIT_COMMON_DIR", "/elsewhere/.git"),
            ("GIT_INDEX_FILE", "/elsewhere/.git/index"),
        ],
    );
    client.initialize();
    let res = client.call_tool("bead_ready", json!({}));
    assert_ne!(res.get("isError").and_then(Value::as_bool), Some(true));

    let log = std::fs::read_to_string(&log_file).expect("read log");
    let canonical_repo = std::fs::canonicalize(&repo).expect("canonicalize");
    for line in log.lines() {
        let entry: Value = serde_json::from_str(line).expect("log line");
        let git_env = entry
            .get("git_env")
            .and_then(Value::as_object)
            .expect("git_env object");
        for routing in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
        ] {
            assert!(
                !git_env.contains_key(routing),
                "ambient {routing} must not reach bead: {entry}"
            );
        }
        let child_cwd =
            std::fs::canonicalize(entry.get("cwd").and_then(Value::as_str).expect("cwd"))
                .expect("canonicalize child cwd");
        assert_eq!(child_cwd, canonical_repo);
    }

    assert_eq!(client.close(), Some(0));
}

#[test]
fn nonexistent_checkout_reports_checkout_unavailable_not_missing_bead() {
    let dir = tempdir().expect("tempdir");
    let missing_repo = dir.path().join("does-not-exist");
    let mut client = McpClient::spawn(
        dir.path(),
        &["--repo", missing_repo.to_str().expect("utf8")],
        &[],
    );
    client.initialize();

    let res = client.call_tool("bead_ready", json!({}));
    assert_eq!(res.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        res.pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("CHECKOUT_UNAVAILABLE")
    );

    assert_eq!(client.close(), Some(0));
}
