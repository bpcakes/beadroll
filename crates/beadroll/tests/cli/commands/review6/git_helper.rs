//! F15: a fetch that times out takes git's helper on the remote side with it.

use std::path::PathBuf;
use std::time::Duration;

use beadroll_test_support::executable;
use beadroll_test_support::{git, within};
use rustix::process::{Pid, Signal, kill_process, test_kill_process};

use super::machine;

/// Whether process `pid` can be signalled, checked without sending a signal.
fn alive(pid: i32) -> bool {
    Pid::from_raw(pid).is_some_and(|pid| test_kill_process(pid).is_ok())
}

/// F15: a fetch that times out does not leave git-upload-pack running.
#[test]
fn f15_timeout_reaps_the_git_helper() {
    let sandbox = crate::sandbox();
    let exec_path = PathBuf::from(git(["--exec-path"], None).trim());
    let fake = sandbox.root().join("git-exec");
    std::fs::create_dir(&fake).unwrap();
    for entry in std::fs::read_dir(&exec_path).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name != "git-upload-pack" && name != "git-receive-pack" {
            std::os::unix::fs::symlink(entry.path(), fake.join(name)).unwrap();
        }
    }
    let log = sandbox.root().join("helper-pids");
    std::fs::write(&log, "").unwrap();
    for name in ["git-upload-pack", "git-receive-pack"] {
        let script = fake.join(name);
        let text = format!(
            "#!/bin/sh\necho $$ >> {}\nsleep 30\nexec {} \"$@\"\n",
            log.display(),
            exec_path.join(name).display()
        );
        executable(&script, text);
    }
    let machine = machine(&sandbox, "f15");
    machine.bead(["create", "one"]);
    let proc = machine
        .cmd(["sync", "--json"])
        .env("BEAD_NET_TIMEOUT", "2")
        .env("GIT_EXEC_PATH", &fake)
        .run();
    assert_ne!(proc.returncode(), 0, "{}", proc.stdout);
    assert!(!proc.crashed(), "{}", proc.stderr);
    let pids: Vec<i32> = std::fs::read_to_string(&log)
        .unwrap()
        .split_whitespace()
        .map(|line| line.parse().unwrap())
        .collect();
    // a helper killed with its group is gone once the system has reaped it
    within(Duration::from_secs(10), || {
        !pids.iter().any(|pid| alive(*pid))
    });
    let survivors: Vec<i32> = pids.into_iter().filter(|pid| alive(*pid)).collect();
    // The `finally`: whatever the assertion says, no helper outlives the test.
    for pid in &survivors {
        if let Some(pid) = Pid::from_raw(*pid) {
            let _ = kill_process(pid, Signal::KILL);
        }
    }
    assert_eq!(survivors, Vec::<i32>::new());
}
