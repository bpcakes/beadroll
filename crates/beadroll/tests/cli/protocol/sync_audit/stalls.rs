//! git stalled in a transport operation, then the sync killed: a push holds the store lock and a
//! local writer waits for it, a fetch holds nothing and a local writer goes ahead.

use std::time::Duration;

use super::{Worker, git_wrapper, setup, two, wait_marker};
use crate::protocol::sync_audit::HookPosition;
use crate::protocol::sync_audit::ProcessSession;

/// How long a process the test waits for may take: `communicate(timeout=5)`, `run(timeout=5)`.
const FIVE_SECONDS: Option<Duration> = Some(Duration::from_secs(5));

/// A sync stalled in its push holds the store lock: a local writer waits for it, and completes
/// once the sync is killed. Nothing is lost, and the next sync leaves nothing waiting.
#[test]
fn slow_push_holds_lock_then_local_writer_completes_without_loss() {
    let sandbox = crate::sandbox();
    let (a, _) = setup(&sandbox);
    two(&sandbox, &a);
    let marker = sandbox.root().join("slow-push");
    a.bead(["create", "before slow push", "--offline"]);
    let path = git_wrapper(&sandbox, "push", &marker, HookPosition::After);
    let mut sync = Worker::start(
        a.cmd(["sync", "--json"]).env("PATH", path),
        ProcessSession::New,
    );
    wait_marker(&marker, &mut sync);
    let writing = a.cmd(["create", "during push", "--offline", "--json"]);
    let mut writer = Worker::start(writing, ProcessSession::Inherited);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        writer.poll().is_none(),
        "writer should wait for the store lock"
    );
    sync.kill().expect("the sync's process group");
    sync.communicate(FIVE_SECONDS);
    let written = writer.communicate(FIVE_SECONDS);
    assert_eq!(written.returncode(), 0, "{}", written.stderr);
    a.bead(["sync"]);
    assert_eq!(a.bead(["list"]).unwrap()["total"].as_i64(), Some(3));
    assert_eq!(
        a.bead(["info"]).unwrap()["waiting_events"].as_i64(),
        Some(0)
    );
}

/// A sync stalled in its first fetch holds no lock: a local write meanwhile completes within five
/// seconds, and once the sync is killed nothing is lost.
#[test]
fn killed_slow_fetch_does_not_block_local_writer() {
    let sandbox = crate::sandbox();
    let (a, _) = setup(&sandbox);
    two(&sandbox, &a);
    let marker = sandbox.root().join("fetching");
    let path = git_wrapper(&sandbox, "fetch", &marker, HookPosition::Before);
    let mut worker = Worker::start(
        a.cmd(["sync", "--json"]).env("PATH", path),
        ProcessSession::New,
    );
    wait_marker(&marker, &mut worker);
    let writing = a.cmd(["create", "during stalled fetch", "--json"]);
    let result = Worker::start(writing, ProcessSession::Inherited).communicate(FIVE_SECONDS);
    assert_eq!(result.returncode(), 0, "{}", result.stderr);
    worker.kill().expect("the sync's process group");
    worker.communicate(FIVE_SECONDS);
    a.bead(["sync"]);
    assert_eq!(a.bead(["list"]).unwrap()["total"].as_i64(), Some(2));
}
