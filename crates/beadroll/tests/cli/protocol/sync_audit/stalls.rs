//! git stalled in a transport operation, then the sync killed: a push holds the store lock and a
//! local writer waits for it, a fetch holds nothing and a local writer goes ahead.

use std::time::Duration;

use super::{git_wrapper, setup, two};
use crate::protocol::sync_audit::HookPosition;
use beadroll_test_support::ProcessGroup;

/// How long a process the test waits for may take: `communicate(timeout=5)`, `run(timeout=5)`.
const FIVE_SECONDS: Duration = Duration::from_secs(5);

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
    let mut sync = a
        .cmd(["sync", "--json"])
        .env("PATH", path)
        .spawn(ProcessGroup::Own);
    sync.wait_for(&marker, Duration::from_secs(15));
    let writing = a.cmd(["create", "during push", "--offline", "--json"]);
    let mut writer = writing.spawn(ProcessGroup::Inherited);
    // An observation window, not a wait for something to happen: the claim is that the writer has
    // NOT finished, which only time can show, since no failpoint fires while a process waits for
    // the store lock. A wait for an event is `wait_until` or a `Gate`, never a sleep.
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
    let mut worker = a
        .cmd(["sync", "--json"])
        .env("PATH", path)
        .spawn(ProcessGroup::Own);
    worker.wait_for(&marker, Duration::from_secs(15));
    let writing = a.cmd(["create", "during stalled fetch", "--json"]);
    let result = writing
        .spawn(ProcessGroup::Inherited)
        .communicate(FIVE_SECONDS);
    assert_eq!(result.returncode(), 0, "{}", result.stderr);
    worker.kill().expect("the sync's process group");
    worker.communicate(FIVE_SECONDS);
    a.bead(["sync"]);
    assert_eq!(a.bead(["list"]).unwrap()["total"].as_i64(), Some(2));
}
