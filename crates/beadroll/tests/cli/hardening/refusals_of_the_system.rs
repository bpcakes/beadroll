//! V6, verified by the seventh review: what the operating system refuses is answered with a
//! structured code and never a crash. An unusable path given on the command line is `IO` and writes
//! nothing, damage to the local store is `IO` or a code of git's, the file-size limit is `IO` (or
//! git's death by SIGXFSZ) with the next write and sync clean, running out of descriptors never
//! drops a pending attempt, and the shapes of `replica.json` and `.beadroll.json` are checked.
//! Scenarios are grouped in modules; their shared helpers are here.

use std::process::Command;
use std::time::Duration;

use beadroll_test_support::{Json, Machine, ProcessGroup, Run, Spawned, UNPROFILED, parse_json};
use rustix::fs::{Access, access};

mod descriptor_exhaustion;
mod file_size_limit;
mod identity_and_config_shapes;
mod store_damage;
mod user_supplied_paths;

/// The code reported by a failed JSON command, `TRACEBACK` for a crash detected by
/// `Run::crashed`, or `None`.
fn error_code(run: &Run) -> Option<String> {
    let reported = parse_json(&run.stderr)
        .ok()
        .and_then(|error| error["error"]["code"].as_str().map(str::to_owned));
    reported.or_else(|| run.crashed().then(|| "TRACEBACK".to_owned()))
}

/// `bead ARGV --json` run as `machine` under a shell resource limit such as `-f 200`
/// or `-n 8`: `/bin/sh -c 'ulimit LIMIT; exec "$@"'` in the machine's checkout with its
/// environment, given 120 seconds. The profile an instrumented bead writes at exit would hit the
/// limit too ([`UNPROFILED`]): these runs write none.
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
        .envs([UNPROFILED]);
    run_within(command, Duration::from_secs(120))
}

/// Runs `command` with empty standard input and reads its output through completion. If it has
/// not ended within `timeout`, it is killed and the test fails.
fn run_within(command: Command, timeout: Duration) -> Run {
    Spawned::start(command, ProcessGroup::Inherited).communicate(timeout)
}

/// Whether the test can run. As root, `chmod 0` does not deny access, so
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

/// Whether the test can run. Without an executable `/bin/sh` there is no shell to
/// apply a ulimit with: the test is skipped, and says so.
fn with_sh() -> bool {
    let found = access("/bin/sh", Access::EXEC_OK).is_ok();
    if !found {
        eprintln!("skipped: no /bin/sh to apply a ulimit with");
    }
    found
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
