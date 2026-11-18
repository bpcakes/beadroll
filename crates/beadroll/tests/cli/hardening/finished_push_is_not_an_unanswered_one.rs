//! G58: a push git finished is not one that got no answer, even when a transport helper's
//! background child still holds git's standard error open.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Instant;

use beadroll_test_support::{Machine, executable};
use rustix::process::{Pid, Signal, kill_process};

/// A transport whose background job keeps git's stderr open after git has
/// exited. It runs the remote command locally; a receive-pack first leaves a `sleep` of
/// `G58_BG_SECONDS` behind, its pid noted in `G58_PIDS`.
const SSH_SCRIPT: &str = r#"#!/bin/sh
if [ "$1" = "-G" ]; then exit 1; fi
for last; do :; done
case "$last" in
  git-receive-pack*) (sleep "$G58_BG_SECONDS") & echo "$!" >> "$G58_PIDS"; eval "exec $last" ;;
  git-upload-pack*) eval "exec $last" ;;
esac
exit 2
"#;

/// How long the helper's background job holds git's stderr, in seconds: well past
/// [`HELD_PAST`], the network timeout a bead waiting on it would run into.
const HELD: &str = "30";

/// The network timeout the test runs with, in seconds: long enough for a push on a loaded
/// machine, short of [`HELD`].
const HELD_PAST: &str = "10";

/// Sends SIGKILL to every helper named in the pid file, when
/// the guard drops, before the sandbox goes.
struct Helpers(PathBuf);

impl Drop for Helpers {
    fn drop(&mut self) {
        let pids = std::fs::read_to_string(&self.0).unwrap_or_default();
        for pid in pids.split_whitespace() {
            if let Some(pid) = pid.parse().ok().and_then(Pid::from_raw) {
                let _ = kill_process(pid, Signal::KILL);
            }
        }
    }
}

/// G58: bead waits for git, not for the end of git's pipes. When a transport helper's
/// background child keeps git's standard error open, a push that git finished (and the remote
/// applied) succeeds at once, instead of running into `BEAD_NET_TIMEOUT`, its exit status
/// thrown away, and `sync` answering `REMOTE_UNREACHABLE` "the push got no answer".
#[test]
fn finished_push_is_not_reported_unanswered_when_a_helper_keeps_stderr() {
    let sandbox = crate::sandbox();
    let pids = sandbox.root().join("g58-helper-pids");
    std::fs::write(&pids, "").unwrap();
    let _helpers = Helpers(pids.clone());
    let ssh = sandbox.root().join("g58-ssh.sh");
    executable(&ssh, SSH_SCRIPT);
    let config = sandbox.root().join("g58-empty.gitconfig");
    std::fs::write(&config, "").unwrap();
    let mut a = Machine::new(&sandbox, "g58");
    let remote = format!("ssh://fakehost{}", sandbox.origin().display());
    let extra: [(&str, OsString); 7] = [
        ("GIT_CONFIG_GLOBAL", config.into_os_string()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_SSH_COMMAND", ssh.into_os_string()),
        ("G58_PIDS", pids.into_os_string()),
        ("G58_BG_SECONDS", "0".into()),
        ("BEAD_NET_TIMEOUT", HELD_PAST.into()),
        ("BEAD_REMOTE", remote.into()),
    ];
    a.extra
        .extend(extra.into_iter().map(|(key, value)| (key.into(), value)));
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "one"]).expect("bead printed a result");
    let key = created["id"].as_str().expect("an id");
    a.bead(["sync"]);
    // the sync below, under the slow helper, publishes it
    a.bead(["comments", "add", key, "publish me", "--offline"]);
    a.extra.insert("G58_BG_SECONDS".into(), HELD.into());
    let before = sandbox.remote_journal();
    let started = Instant::now();
    let synced = a.run(["sync", "--json"]);
    let took = started.elapsed();
    assert_ne!(
        sandbox.remote_journal(),
        before,
        "the push did not reach the remote"
    );
    assert_eq!(
        synced.returncode(),
        0,
        "git finished and the remote applied the push, but after {:.1}s bead said: {}",
        took.as_secs_f64(),
        synced.stderr.trim()
    );
    // and no time bound: the helper holds git's stderr past `BEAD_NET_TIMEOUT`, so a bead that
    // waited for it would have answered `REMOTE_UNREACHABLE` above, however fast the machine
}
