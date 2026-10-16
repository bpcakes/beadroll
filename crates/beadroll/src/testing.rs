//! What the unit tests that run bead in this process share: a machine of the sandbox with a
//! tracker, its store opened as its runs open it, and its actor.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;

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

/// The actor of the machine's in-process runs.
pub(crate) fn actor(machine: &Machine) -> Actor {
    let thread = format!("{}-thread-0001", machine.name());
    Actor::writable(json!({"agent": "codex", "thread": thread}))
}

/// A machine with a tracker (prefix `ex`), in a sandbox whose remote it publishes to.
pub(crate) fn tracker() -> (Sandbox, Machine) {
    let sandbox = Sandbox::new(built_bead());
    let machine = Machine::new(&sandbox, "a");
    machine.bead(["init", "--prefix", "ex"]);
    (sandbox, machine)
}
