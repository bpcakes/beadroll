//! What the unit tests that run bead in this process share: a machine of the sandbox with a
//! tracker, its store opened as its runs open it, and its actor.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use beadroll_core::json::Object;
use beadroll_store::{Actor, Settings, Workspace};
use beadroll_test_support::{Machine, Sandbox, built_bead, json};

/// The settings of `machine`'s runs, from its checkout, with `vars` set besides.
pub(crate) fn settings(machine: &Machine, vars: &[(&str, &str)]) -> Arc<Settings> {
    let mut env: BTreeMap<OsString, OsString> = machine.env(None, "codex");
    for (name, value) in vars {
        env.insert((*name).into(), (*value).into());
    }
    let settings = Settings::from_process(
        env,
        Some(machine.checkout().to_path_buf()),
        std::env::var_os("HOME").unwrap_or_default(),
        std::env::temp_dir(),
        &|_| None,
    );
    Arc::new(settings.expect("the machine's settings"))
}

/// Opens the machine's store in-process, as its runs open it, with `vars` in its environment.
pub(crate) fn workspace(machine: &Machine, origin: &Path, vars: &[(&str, &str)]) -> Workspace {
    let store = machine.store();
    let id = store
        .file_name()
        .and_then(OsStr::to_str)
        .expect("a store id");
    let origin = Some(origin.as_os_str().to_owned());
    Workspace::new(settings(machine, vars), id, None, origin, Object::new()).expect("the store")
}

/// Opens the machine's store as a checkout without an origin opens it, with `config` as what its
/// `.beadroll.json` tracks.
pub(crate) fn without_origin(machine: &Machine, config: Object) -> Workspace {
    let store = machine.store();
    let id = store
        .file_name()
        .and_then(OsStr::to_str)
        .expect("a store id");
    Workspace::new(settings(machine, &[]), id, None, None, config).expect("the store")
}

/// The actor of the machine's in-process runs.
pub(crate) fn actor(machine: &Machine) -> Actor {
    let thread = format!("{}-thread-0001", machine.name());
    Actor::writable(json!({"agent": "codex", "thread": thread}))
}

/// A machine with a tracker (prefix `ex`), in a sandbox whose remote it publishes to. It runs
/// this build's bead whatever `BEAD_UNDER_TEST` says, since the tests read its store in process.
pub(crate) fn tracker() -> (Sandbox, Machine) {
    let sandbox = Sandbox::new(built_bead());
    let machine = Machine::new(&sandbox, "a").with_bead(built_bead());
    machine.bead(["init", "--prefix", "ex"]);
    (sandbox, machine)
}

/// The handshake of a `pause-until` failpoint (`crates/beadroll-store/src/failpoint.rs`): the
/// paused run touches `PATH.ready`, then waits for `PATH.go`. Once it has reached the gate (or
/// `done` says it will not), runs `step`, then opens the gate, whatever `step` came to.
pub(crate) fn meet<T>(gate: &Path, done: &dyn Fn() -> bool, step: impl FnOnce() -> T) -> Option<T> {
    struct Open(PathBuf);
    impl Drop for Open {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.0, b"");
        }
    }
    let with = |suffix: &str| {
        let mut name = gate.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let _open = Open(with(".go"));
    let deadline = Instant::now() + Duration::from_secs(120);
    while !with(".ready").exists() {
        if done() || Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Some(step())
}
