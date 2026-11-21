//! End-to-end tests running `beadroll-mcp` against the real workspace-built `bead` binary in
//! isolated `beadroll-test-support::Sandbox` repositories.

use beadroll_test_support::{
    Json, Machine, Sandbox, built_bead, converge, executable, git, path_text, shell_quoted,
};
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::McpClient;

fn sandbox() -> Sandbox {
    Sandbox::new(built_bead())
}

fn spawn_for_machine(m: &Machine, extra_env: &[(&str, &str)]) -> McpClient {
    let bead_path = m.executable().to_path_buf();
    let home = path_text(m.home()).to_owned();
    let repo = path_text(m.checkout()).to_owned();
    let mut env = vec![("BEAD_HOME", home.as_str())];
    env.extend_from_slice(extra_env);
    let mut client = McpClient::spawn(
        m.checkout(),
        &["--repo", &repo, "--bead", path_text(&bead_path)],
        &env,
    );
    client.initialize();
    client
}

fn find_row_by_title(listed: &Value, title: &str) -> (String, String) {
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
fn real_bead_uninitialized_and_populated_reads_never_contact_remote() {
    let sb = sandbox();
    let m = Machine::new(&sb, "a");
    let mut client = spawn_for_machine(&m, &[]);

    let uninit = client.call_tool("bead_ready", json!({}));
    assert_eq!(uninit.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        uninit
            .pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("NOT_INITIALIZED")
    );
    assert!(sb.remote_journal().is_none());

    m.bead(["init", "--prefix", "mcp"]);
    let created = m
        .bead(["create", "Verify zero remote contact"])
        .expect("create");
    let created_uid = created
        .get("uid")
        .and_then(Json::as_str)
        .expect("uid")
        .to_owned();

    // Install a git uploadpack sentinel on origin without altering `remote.origin.url` so the
    // workspace ID stays unchanged, while any remote fetch attempt immediately creates the marker.
    let marker = sb.root().join("remote-contacted");
    let sentinel = sb.root().join("uploadpack-sentinel.sh");
    let body = format!(
        "#!/bin/sh\ntouch {}\nexit 99\n",
        shell_quoted(path_text(&marker))
    );
    executable(&sentinel, body);
    let _ = git(
        ["config", "remote.origin.uploadpack", path_text(&sentinel)],
        Some(m.checkout()),
    );

    let ready = client.call_tool("bead_ready", json!({}));
    assert_ne!(ready.get("isError").and_then(Value::as_bool), Some(true));
    let ws_id = ready
        .pointer("/structuredContent/workspace/id")
        .and_then(Value::as_str)
        .expect("ws id")
        .to_owned();

    let detail = client.call_tool(
        "bead_show",
        json!({ "uid": created_uid, "workspace": ws_id }),
    );
    assert_ne!(detail.get("isError").and_then(Value::as_bool), Some(true));
    assert!(
        !marker.exists(),
        "local MCP reads must never contact the remote transport"
    );

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
fn real_bead_renumbered_child_key_reopens_original_issue_by_stable_uid() {
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
    assert!(
        parent.get("uid").and_then(Json::as_str).is_some(),
        "workspace bead must emit stable uids"
    );
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

    let mut client_a = spawn_for_machine(&m1, &[]);
    let listed_a = client_a.call_tool("bead_ready", json!({}));
    assert_ne!(listed_a.get("isError").and_then(Value::as_bool), Some(true));
    let ws_id = listed_a
        .pointer("/structuredContent/workspace/id")
        .and_then(Value::as_str)
        .expect("workspace id")
        .to_owned();
    let (pre_key_a, uid_a) = find_row_by_title(&listed_a, "Child from A");

    let mut client_b = spawn_for_machine(&m2, &[]);
    let listed_b = client_b.call_tool("bead_ready", json!({}));
    let (pre_key_b, uid_b) = find_row_by_title(&listed_b, "Child from B");
    assert_eq!(pre_key_a, pre_key_b);

    // Converge both machines: one colliding child key is renumbered from `.1` to `.2`.
    converge(&[&m1, &m2]);

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

#[test]
fn real_bead_inherited_workspace_and_git_pushurl_cannot_hijack_bound_tracker() {
    let sb_a = sandbox();
    let sb_b = sandbox();
    let shared_home = tempdir().expect("shared BEAD_HOME");
    let home = path_text(shared_home.path()).to_owned();

    let mut ma = Machine::new(&sb_a, "a");
    let mut mb = Machine::new(&sb_b, "b");
    ma.extra.insert("BEAD_HOME".into(), home.clone().into());
    mb.extra.insert("BEAD_HOME".into(), home.clone().into());

    ma.bead(["init", "--prefix", "dup"]);
    ma.bead(["create", "Only in Tracker A"]);
    mb.bead(["init", "--prefix", "dup"]);
    mb.bead(["create", "Only in Tracker B"]);

    let info_b = mb.bead(["info"]).expect("info b");
    let ws_b = info_b
        .get("workspace")
        .and_then(Json::as_str)
        .expect("ws_b")
        .to_owned();
    let origin_b = path_text(sb_b.origin()).to_owned();
    let global_cfg = shared_home.path().join("evil-global.gitconfig");
    std::fs::write(
        &global_cfg,
        format!("[remote \"origin\"]\n\tpushurl = {origin_b}\n"),
    )
    .expect("write global gitconfig");
    let xdg_dir = shared_home.path().join("xdg");
    let xdg_git = xdg_dir.join("git");
    std::fs::create_dir_all(&xdg_git).expect("xdg git dir");
    std::fs::write(
        xdg_git.join("config"),
        format!("[remote \"origin\"]\n\tpushurl = {origin_b}\n"),
    )
    .expect("write xdg gitconfig");

    // Launch `beadroll-mcp` bound to checkout A with file-backed GIT_CONFIG_GLOBAL,
    // XDG_CONFIG_HOME, GIT_CONFIG_COUNT pushurl, BEAD_WORKSPACE, malformed BEAD_NET_TIMEOUT, and
    // BEAD_FAILPOINTS all trying to hijack or break reads of Tracker A.
    let mut client = McpClient::spawn(
        ma.checkout(),
        &[
            "--repo",
            path_text(ma.checkout()),
            "--bead",
            path_text(ma.executable()),
        ],
        &[
            ("BEAD_HOME", &home),
            ("BEAD_WORKSPACE", &ws_b),
            ("BEAD_NET_TIMEOUT", "not-a-number"),
            ("BEAD_FAILPOINTS", "projection.read=exit(99)"),
            ("GIT_CONFIG_GLOBAL", path_text(&global_cfg)),
            ("XDG_CONFIG_HOME", path_text(&xdg_dir)),
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "remote.origin.pushurl"),
            ("GIT_CONFIG_VALUE_0", &origin_b),
        ],
    );
    client.initialize();

    let listed = client.call_tool("bead_ready", json!({}));
    assert_ne!(listed.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        listed
            .pointer("/structuredContent/issues/0/title")
            .and_then(Value::as_str),
        Some("Only in Tracker A"),
        "inherited env redirected read to Tracker B: {listed}"
    );

    let cross = client.call_tool("bead_show", json!({ "id": "dup-1", "workspace": ws_b }));
    assert_eq!(cross.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        cross
            .pointer("/structuredContent/error/code")
            .and_then(Value::as_str),
        Some("WORKSPACE_MISMATCH")
    );

    assert_eq!(client.close(), Some(0));
}

#[test]
fn real_bead_show_closed_issue_in_partial_clone_never_triggers_lazy_fetch() {
    let sb = sandbox();
    let _ = git(
        ["config", "uploadpack.allowFilter", "true"],
        Some(sb.origin()),
    );

    let m = Machine::new(&sb, "writer");
    m.bead(["init", "--prefix", "pcl"]);

    // Close an issue on a separate branch commit that the partial clone will not fetch.
    let _ = git(["checkout", "-b", "topic-close"], Some(m.checkout()));
    std::fs::write(m.checkout().join("topic.txt"), "closed work\n").expect("write topic");
    let _ = git(["add", "topic.txt"], Some(m.checkout()));
    let _ = git(["commit", "-m", "topic work"], Some(m.checkout()));
    let _ = git(
        ["push", "origin", "HEAD:refs/heads/topic-close"],
        Some(m.checkout()),
    );

    let created = m
        .bead(["create", "Closed on topic branch"])
        .expect("create");
    let uid = created
        .get("uid")
        .and_then(Json::as_str)
        .expect("uid")
        .to_owned();
    m.bead(["close", &uid, "-r", "done on topic"]);

    // Partial clone (`--filter=tree:0 --single-branch --branch main`) lacks the closing commit.
    let partial = sb.root().join("partial-clone");
    let origin_url = format!("file://{}", path_text(sb.origin()));
    let _ = git(
        [
            "clone",
            "--filter=tree:0",
            "--single-branch",
            "--branch",
            "main",
            &origin_url,
            path_text(&partial),
        ],
        None,
    );

    // Sync tracker in the partial clone's own home BEFORE installing the sentinel.
    let home = sb.root().join("home-partial");
    let sync_status = std::process::Command::new(m.executable())
        .args(["sync"])
        .current_dir(&partial)
        .env("BEAD_HOME", &home)
        .status()
        .expect("sync partial");
    assert!(sync_status.success(), "bead sync in partial clone");

    // Install uploadpack sentinel: any lazy promisor fetch during `bead show` creates `marker`.
    let marker = sb.root().join("partial-lazy-fetched");
    let sentinel = sb.root().join("partial-uploadpack.sh");
    let body = format!(
        "#!/bin/sh\ntouch {}\nexec git-upload-pack \"$@\"\n",
        shell_quoted(path_text(&marker))
    );
    executable(&sentinel, body);
    let _ = git(
        ["config", "remote.origin.uploadpack", path_text(&sentinel)],
        Some(&partial),
    );

    let mut client = McpClient::spawn(
        &partial,
        &[
            "--repo",
            path_text(&partial),
            "--bead",
            path_text(m.executable()),
        ],
        &[("BEAD_HOME", path_text(&home))],
    );
    client.initialize();

    let shown = client.call_tool("bead_show", json!({ "uid": uid }));
    assert_ne!(shown.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(
        shown
            .pointer("/structuredContent/issue/status")
            .and_then(Value::as_str),
        Some("closed")
    );
    assert!(
        !marker.exists(),
        "bead_show triggered a lazy promisor fetch in a partial clone"
    );

    assert_eq!(client.close(), Some(0));
}
