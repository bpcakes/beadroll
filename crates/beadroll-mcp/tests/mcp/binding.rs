//! Checkout session binding, symlink canonicalization at startup, `info`-to-read workspace
//! pinning, isolation between trackers sharing a display key, and scrubbing of inherited git and
//! beadroll routing environment variables.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tempfile::tempdir;

use crate::{McpClient, write_stateful_fake_bead};

fn write_tracker_state(
    dir: &Path,
    ws_id: &str,
    name: &str,
    uid: &str,
    title: &str,
) -> (PathBuf, PathBuf) {
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
fn ambient_git_and_bead_routing_vars_are_scrubbed_and_reads_pin_info_workspace() {
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
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "remote.origin.pushurl"),
            ("GIT_CONFIG_VALUE_0", "https://example.invalid/hijack.git"),
            ("GIT_CEILING_DIRECTORIES", "/"),
            ("BEAD_WORKSPACE", "hijacked-workspace"),
            ("BEAD_REMOTE", "hijacked-remote"),
            ("BEAD_HOME", "/tmp/preserved-bead-home"),
        ],
    );
    client.initialize();
    let res = client.call_tool("bead_ready", json!({}));
    assert_ne!(res.get("isError").and_then(Value::as_bool), Some(true));

    let log = std::fs::read_to_string(&log_file).expect("read log");
    let entries: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).expect("log line"))
        .collect();
    assert_eq!(entries.len(), 2, "expected info then ready");

    let canonical_repo = std::fs::canonicalize(&repo).expect("canonicalize");
    for entry in &entries {
        let git_env = entry
            .get("git_env")
            .and_then(Value::as_object)
            .expect("git_env");
        for scrubbed in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_VALUE_0",
            "GIT_CEILING_DIRECTORIES",
        ] {
            assert!(
                !git_env.contains_key(scrubbed),
                "ambient {scrubbed} leaked to bead: {entry}"
            );
        }
        let bead_env = entry
            .get("bead_env")
            .and_then(Value::as_object)
            .expect("bead_env");
        assert!(!bead_env.contains_key("BEAD_REMOTE"));
        assert_eq!(
            bead_env.get("BEAD_HOME").and_then(Value::as_str),
            Some("/tmp/preserved-bead-home")
        );
        let child_cwd =
            std::fs::canonicalize(entry.get("cwd").and_then(Value::as_str).expect("cwd"))
                .expect("canonicalize child cwd");
        assert_eq!(child_cwd, canonical_repo);
    }

    // `bead info` must NOT receive inherited BEAD_WORKSPACE, whereas `bead ready` MUST be pinned
    // to the exact workspace ID `bead info` returned.
    assert!(entries[0].pointer("/bead_env/BEAD_WORKSPACE").is_none());
    assert_eq!(
        entries[1]
            .pointer("/bead_env/BEAD_WORKSPACE")
            .and_then(Value::as_str),
        Some("ws-clean")
    );

    assert_eq!(client.close(), Some(0));
}

#[test]
fn retargeting_a_repo_symlink_after_startup_cannot_redirect_session() {
    let dir = tempdir().expect("tempdir");
    let repo_a = dir.path().join("repo-a");
    let repo_b = dir.path().join("repo-b");
    std::fs::create_dir(&repo_a).expect("repo_a");
    std::fs::create_dir(&repo_b).expect("repo_b");
    let link = dir.path().join("active-checkout");
    symlink(&repo_a, &link).expect("symlink to repo_a");

    let (fake_bead, log_file) = write_tracker_state(
        &repo_a,
        "ws-a",
        "proj-a",
        "019a0000-0000-7000-8000-111111111111",
        "Issue A",
    );

    let mut client = McpClient::spawn(
        dir.path(),
        &[
            "--repo",
            link.to_str().expect("utf8"),
            "--bead",
            fake_bead.to_str().expect("utf8"),
        ],
        &[],
    );
    client.initialize();

    // Retarget the symlink to repo_b after the server started.
    std::fs::remove_file(&link).expect("remove old symlink");
    symlink(&repo_b, &link).expect("retarget symlink to repo_b");

    let res = client.call_tool("bead_ready", json!({}));
    assert_ne!(res.get("isError").and_then(Value::as_bool), Some(true));

    let canonical_a = std::fs::canonicalize(&repo_a).expect("canonicalize repo_a");
    let log = std::fs::read_to_string(&log_file).expect("read log");
    for line in log.lines() {
        let entry: Value = serde_json::from_str(line).expect("json");
        let child_cwd =
            std::fs::canonicalize(entry.get("cwd").and_then(Value::as_str).expect("cwd"))
                .expect("canonicalize");
        assert_eq!(
            child_cwd, canonical_a,
            "symlink retarget redirected bead subprocess to another directory"
        );
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
