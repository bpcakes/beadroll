//! V6, verified by the seventh review: what the operating system refuses is answered with a
//! structured code and never a crash. An unusable path given on the command line is `IO` and writes
//! nothing, damage to the local store is `IO` or a code of git's, the file-size limit is `IO` (or
//! git's death by SIGXFSZ) with the next write and sync clean, running out of descriptors never
//! drops a pending attempt, and the shapes of `replica.json` and `.beadroll.json` are checked.
//! Scenarios are grouped in modules; their shared helpers are here.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use beadroll_test_support::{Json, Machine, Run, parse_json};
use rustix::fs::{Access, access};

mod v6_descriptor_exhaustion;
mod v6_file_size_limit;
mod v6_identity_and_config_shapes;
mod v6_store_damage;
mod v6_user_supplied_paths;

/// The code reported by a failed JSON command, `TRACEBACK` for a crash detected by
/// `Run::crashed`, or `None`.
fn error_code(run: &Run) -> Option<String> {
    let reported = parse_json(&run.stderr)
        .ok()
        .and_then(|error| error["error"]["code"].as_str().map(str::to_owned));
    reported.or_else(|| run.crashed().then(|| "TRACEBACK".to_owned()))
}

/// `_v6_limited`: `bead ARGV --json` run as `machine` under a shell resource limit such as `-f 200`
/// or `-n 8`: `/bin/sh -c 'ulimit LIMIT; exec "$@"'` in the machine's checkout with its
/// environment, given 120 seconds. Under `cargo xtask coverage` the profile bead would write at
/// exit hits a file-size limit too, and says so on standard error: these runs write none.
fn limited(machine: &Machine, limit: &str, argv: &[&str]) -> Run {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", &format!("ulimit {limit}; exec \"$@\""), "sh"])
        .arg(machine.executable())
        .args(argv)
        .arg("--json")
        .current_dir(machine.checkout())
        .env_clear()
        .envs(machine.env(None, "codex"))
        .env("LLVM_PROFILE_FILE", "/dev/null");
    run_within(command, Duration::from_secs(120))
}

/// Runs a command with empty standard input and reads output through completion. If either
/// process or output exceeds `timeout`, kill it and fail the test.
fn run_within(mut command: Command, timeout: Duration) -> Run {
    let deadline = Instant::now() + timeout;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("cannot start {command:?}: {error}"));
    let stdout = drain(child.stdout.take().expect("the standard output"));
    let stderr = drain(child.stderr.take().expect("the standard error"));
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{command:?} had not ended after {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let read = |pipe: &Receiver<Vec<u8>>| {
        let left = deadline.saturating_duration_since(Instant::now());
        let bytes = pipe.recv_timeout(left);
        let bytes = bytes.unwrap_or_else(|_| panic!("{command:?}'s output had not ended"));
        String::from_utf8(bytes).expect("UTF-8 output")
    };
    Run {
        status,
        stdout: read(&stdout),
        stderr: read(&stderr),
    }
}

/// A thread that reads `pipe` to its end, and the channel it hands what it read to.
fn drain(mut pipe: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
}

/// `_v6_need_unprivileged`: whether the test can run. As root, `chmod 0` does not deny access, so
/// EACCES cannot be produced: the test is skipped, and says so.
fn unprivileged() -> bool {
    let root = rustix::process::geteuid().is_root();
    if root {
        eprintln!(
            "skipped: runs as root: chmod 0 does not deny access, so EACCES cannot be produced"
        );
    }
    !root
}

/// `_v6_need_sh`: whether the test can run. Without an executable `/bin/sh` there is no shell to
/// apply a ulimit with: the test is skipped, and says so.
fn with_sh() -> bool {
    let found = access("/bin/sh", Access::EXEC_OK).is_ok();
    if !found {
        eprintln!("skipped: no /bin/sh to apply a ulimit with");
    }
    found
}

/// The id of the issue a `--json` result is: `result["id"]`.
fn id(result: Option<Json>) -> String {
    let result = result.expect("bead printed a result");
    result["id"].as_str().expect("an id").to_owned()
}

/// The member `key` of `value`; absence fails the test.
fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
}

/// The machine id from replica.json.
fn replica_id(machine: &Machine) -> Json {
    let text = std::fs::read_to_string(machine.home().join("replica.json")).unwrap();
    let identity = parse_json(&text).unwrap();
    member(&identity, "id").clone()
}
