//! Waiting for something a test cannot be told about directly: a condition polled with a deadline,
//! which fails the test when the deadline passes, rather than a fixed sleep that only hopes. A
//! wait that bead itself can be held for is a failpoint's `pause-until` instead ([`crate::Gate`]).

use std::process::Command;
use std::time::{Duration, Instant};

use crate::Sandbox;

/// How long a test waits for its remote to settle: a receive side stalled in a hook a test has
/// released finishes in well under a second, so twenty is a hang, not a slow machine.
pub const REMOTE_SETTLES: Duration = Duration::from_secs(20);

/// Polls `done` every 10 ms until it holds; panics, naming `what`, if `limit` passes first.
pub fn wait_until(what: &str, limit: Duration, done: impl FnMut() -> bool) {
    assert!(within(limit, done), "waited {limit:?} for {what}, in vain");
}

/// Polls `done` every 10 ms until it holds or `limit` passes: whether it held. For a test that
/// still has to clean up when it did not, before it fails.
pub fn within(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Sandbox {
    /// Whether a `git receive-pack` is still serving the sandbox's remote. A push bead gave up on
    /// (`BEAD_NET_TIMEOUT`) goes on at the remote until its hooks return, and then lands or fails.
    pub fn receive_pack_running(&self) -> bool {
        let pattern = format!("receive-pack.*{}", self.origin().display());
        let found = Command::new("pgrep")
            .args(["-f", &pattern])
            .output()
            .expect("pgrep runs");
        !String::from_utf8_lossy(&found.stdout).trim().is_empty()
    }

    /// Waits until no receive-pack serves the remote, so that every push still in flight has
    /// landed or failed and the remote stays as the test then reads it. Panics after
    /// [`REMOTE_SETTLES`].
    pub fn await_remote_settled(&self) {
        wait_until(
            "the remote's receive-packs to finish",
            REMOTE_SETTLES,
            || !self.receive_pack_running(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_condition_that_holds_returns_at_once() {
        let started = Instant::now();
        wait_until("nothing", Duration::from_secs(5), || true);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    #[should_panic(expected = "waited 50ms for the impossible, in vain")]
    fn a_condition_that_never_holds_fails_the_test_naming_it() {
        wait_until("the impossible", Duration::from_millis(50), || false);
    }

    /// A push held in the remote's pre-receive hook is seen running, and the remote is settled
    /// once the hook is let go. Any file stands in for bead, since no machine runs here.
    #[test]
    fn a_push_held_at_the_remote_is_running_until_its_hook_returns() {
        let this = std::env::current_exe().expect("the running test's own executable");
        let sandbox = Sandbox::selecting(this, None);
        assert!(!sandbox.receive_pack_running());
        let release = sandbox.root().join("release");
        let hook = sandbox.origin().join("hooks").join("pre-receive");
        let script = format!(
            "#!/bin/sh\nwhile [ ! -f '{}' ]; do sleep 0.05; done\nexit 0\n",
            release.display()
        );
        crate::executable(&hook, script);
        let seed = sandbox.root().join("seed");
        let identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
        crate::git(
            identity
                .into_iter()
                .chain(["commit", "-q", "--allow-empty", "-m", "held"]),
            Some(&seed),
        );
        let mut push = crate::git_command()
            .args(["push", "-q", "origin", "HEAD:main"])
            .current_dir(&seed)
            .spawn()
            .expect("git push starts");
        wait_until("the push to reach the hook", REMOTE_SETTLES, || {
            sandbox.receive_pack_running()
        });
        std::fs::write(&release, b"").expect("the release");
        sandbox.await_remote_settled();
        assert!(push.wait().expect("git push ends").success());
    }
}
