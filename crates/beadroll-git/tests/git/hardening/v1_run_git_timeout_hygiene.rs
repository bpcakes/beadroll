//! V1: in-process runner timeouts leak no descriptor and no zombie, and reap as documented (F15).

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt as _;
use std::process::Command;
use std::time::Duration;

use beadroll_git::{Env, Error};

fn alive(pid: i32) -> bool {
    rustix::process::Pid::from_raw(pid)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

fn kill(pid: i32) {
    if let Some(pid) = rustix::process::Pid::from_raw(pid) {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    }
}

fn open_descriptors() -> usize {
    std::fs::read_dir("/dev/fd").unwrap().count()
}

fn zombies() -> Vec<String> {
    let ps = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,stat="])
        .output()
        .unwrap();
    let me = std::process::id().to_string();
    String::from_utf8_lossy(&ps.stdout)
        .lines()
        .filter(|row| {
            let fields: Vec<&str> = row.split_whitespace().collect();
            fields.len() >= 3 && fields[1] == me && fields[2].contains('Z')
        })
        .map(str::to_owned)
        .collect()
}

/// Timed-out runs report a timeout, leak no file descriptor and leave no zombie; reap (a fetch) kills
/// the helpers git started, no reap (a push) leaves them running, as F15 documents.
#[test]
fn timeouts_leak_nothing_and_reap_as_documented() {
    if !std::path::Path::new("/dev/fd").is_dir() {
        return; // no /dev/fd to count this process's descriptors
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let log = root.join("helper-pids");
    let helper = root.join("stall.sh");
    std::fs::write(
        &helper,
        format!("#!/bin/sh\necho $$ >> '{}'\nexec sleep 3\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let env = Env::new(std::env::vars_os(), None, std::env::temp_dir());
    let args: Vec<OsString> = vec![
        "-C".into(),
        root.into_os_string(),
        "-c".into(),
        format!("alias.stall=!'{}'", helper.display()).into(),
        "stall".into(),
    ];
    let mut survivors = Vec::new();
    for (reap, expect_left) in [(true, false), (false, true)] {
        std::fs::write(&log, "").unwrap();
        let before = open_descriptors();
        // The timeout starts after spawning git. An aggregate wall-clock limit would also
        // measure process startup, which is slow on macOS CI. Check each timeout result and
        // the helper lifetimes below; nextest's slow-timeout still bounds a hung test.
        for _ in 0..20 {
            let error = env
                .git(&args)
                .timeout(0.2)
                .reap(reap)
                .run()
                .expect_err("the alias outlives its time");
            assert!(matches!(error, Error::Timeout { .. }), "{error:?}");
            assert_eq!(error.to_string(), "git -C gave no answer within 0.2s");
        }
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(open_descriptors(), before, "reap={reap}");
        assert_eq!(zombies(), Vec::<String>::new(), "reap={reap}");
        let started_helpers: Vec<i32> = std::fs::read_to_string(&log)
            .unwrap()
            .split_whitespace()
            .map(|pid| pid.parse().unwrap())
            .collect();
        assert!(
            !started_helpers.is_empty(),
            "no helper started within the timeout"
        );
        let left: Vec<i32> = started_helpers
            .into_iter()
            .filter(|pid| alive(*pid))
            .collect();
        assert_eq!(!left.is_empty(), expect_left, "reap={reap}: {left:?}");
        survivors.extend(left);
    }
    for pid in survivors {
        kill(pid);
    }
}
