//! End-to-end tests running `beadroll-mcp` against the real `bead` binary in isolated
//! `beadroll-test-support::Sandbox` repositories.

use beadroll_test_support::{Json, Machine, Sandbox, built_bead, converge, path_text};
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::McpClient;

fn sandbox() -> Sandbox {
    Sandbox::new(built_bead())
}

fn spawn_for_machine(m: &Machine) -> McpClient {
    let bead_path = m.executable().to_path_buf();
    let home = path_text(m.home()).to_owned();
    let repo = path_text(m.checkout()).to_owned();
    let mut client = McpClient::spawn(
        m.checkout(),
        &["--repo", &repo, "--bead", path_text(&bead_path)],
        &[("BEAD_HOME", &home)],
    );
    client.initialize();
    client
}

fn find_uid_by_title(listed: &Value, title: &str) -> (String, String) {
    let row = listed
        .pointer("/structuredContent/issues")
        .and_then(Value::as_array)
        .expect("issues array")
        .iter()
        .find(|row| row.get("title").and_then(Value::as_str) == Some(title))
        .unwrap_or_else(|| panic!("missing row titled {title:?} in {listed}"));
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .expect("id")
        .to_owned();
    let uid = row
        .get("uid")
        .and_then(Value::as_str)
        .expect("uid")
        .to_owned();
    (id, uid)
}

#[test]
fn real_bead_uninitialized_and_empty_ready_perform_local_reads_only() {
    let sb = sandbox();
    let m = Machine::new(&sb, "a");
    let mut client = spawn_for_machine(&m);

    // Uninitialized checkout reports NOT_INITIALIZED and touches nothing on the remote.
    let uninit = client.call_tool("bead_ready", json!({}));
    assert_eq!(uninit.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        uninit
            .pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("NOT_INITIALIZED")
    );
    assert!(sb.remote_journal().is_none());

    // Initialize an empty tracker, then refresh via `bead_ready`.
    m.bead(["init", "--prefix", "mcp"]);
    let head_before = sb.remote_journal().expect("remote initialized");

    let empty = client.call_tool("bead_ready", json!({}));
    assert_ne!(empty.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        empty
            .pointer("/structuredContent/issues")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        empty
            .pointer("/structuredContent/workspace/prefix")
            .and_then(Value::as_str),
        Some("mcp")
    );
    assert_eq!(sb.remote_journal(), Some(head_before));

    assert_eq!(client.close(), Some(0));
}

#[test]
fn real_bead_non_git_directory_reports_no_workspace() {
    let sb = sandbox();
    let m = Machine::new(&sb, "a");
    let bead_path = m.executable().to_path_buf();
    let home = path_text(m.home()).to_owned();
    let plain_dir = tempdir().expect("non-git dir");

    let mut client = McpClient::spawn(
        plain_dir.path(),
        &[
            "--repo",
            path_text(plain_dir.path()),
            "--bead",
            path_text(&bead_path),
        ],
        &[("BEAD_HOME", &home)],
    );
    client.initialize();

    let res = client.call_tool("bead_ready", json!({}));
    assert_eq!(res.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        res.pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("NO_WORKSPACE")
    );

    assert_eq!(client.close(), Some(0));
}

#[test]
fn real_bead_browsing_and_renumbered_child_key_or_upgrade_contract() {
    let sb = sandbox();
    let m1 = Machine::new(&sb, "a");
    let m2 = Machine::new(&sb, "b");
    m1.bead(["init", "--prefix", "mcp"]);
    m2.bead(["sync"]);

    let parent = m1
        .bead(["create", "Parent epic", "-t", "epic"])
        .expect("create parent");
    let parent_id = parent
        .get("id")
        .and_then(Json::as_str)
        .expect("parent id")
        .to_owned();
    let supports_uid = parent.get("uid").and_then(Json::as_str).is_some();
    m2.bead(["sync"]);

    // Both machines create a child offline under the same parent so both mint `<parent>.1`.
    let child_a = m1
        .bead([
            "create",
            "Child from A",
            "--parent",
            &parent_id,
            "--offline",
        ])
        .expect("create child_a");
    let child_b = m2
        .bead([
            "create",
            "Child from B",
            "--parent",
            &parent_id,
            "--offline",
        ])
        .expect("create child_b");
    assert_eq!(child_a.get("id"), child_b.get("id"));

    let mut client_a = spawn_for_machine(&m1);
    let listed_a = client_a.call_tool("bead_ready", json!({}));
    if !supports_uid {
        assert_eq!(listed_a.get("isError").and_then(Value::as_bool), Some(true));
        assert_eq!(
            listed_a
                .pointer("/structuredContent/error/code")
                .and_then(Value::as_str),
            Some("UPGRADE_REQUIRED")
        );
        assert_eq!(client_a.close(), Some(0));
        return;
    }

    let ws_id = listed_a
        .pointer("/structuredContent/workspace/id")
        .and_then(Value::as_str)
        .expect("workspace id")
        .to_owned();
    let (pre_key_a, uid_a) = find_uid_by_title(&listed_a, "Child from A");

    let mut client_b = spawn_for_machine(&m2);
    let listed_b = client_b.call_tool("bead_ready", json!({}));
    let (pre_key_b, uid_b) = find_uid_by_title(&listed_b, "Child from B");
    assert_eq!(pre_key_a, pre_key_b);

    // Converge both machines: one colliding child key is renumbered from `.1` to `.2`.
    converge(&[&m1, &m2]);

    // Clicking both pre-sync rows by `(uid, workspace)` opens each original issue accurately.
    let detail_a = client_a.call_tool("bead_show", json!({ "uid": uid_a, "workspace": ws_id }));
    let detail_b = client_a.call_tool("bead_show", json!({ "uid": uid_b, "workspace": ws_id }));
    assert_eq!(
        detail_a
            .pointer("/structuredContent/issue/title")
            .and_then(Value::as_str),
        Some("Child from A")
    );
    assert_eq!(
        detail_b
            .pointer("/structuredContent/issue/title")
            .and_then(Value::as_str),
        Some("Child from B")
    );
    assert_ne!(
        detail_a
            .pointer("/structuredContent/issue/id")
            .and_then(Value::as_str),
        detail_b
            .pointer("/structuredContent/issue/id")
            .and_then(Value::as_str),
        "sync must have renumbered one colliding child key"
    );

    assert_eq!(client_a.close(), Some(0));
    assert_eq!(client_b.close(), Some(0));
}
