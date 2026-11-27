//! What the unit tests that run bead in this process share: a machine of the sandbox with a
//! tracker, its store opened as its runs open it, its actor, and a probe that counts its frees.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use beadroll_core::json::Object;
use beadroll_store::{Actor, Settings, Workspace};
use beadroll_test_support::{Machine, Sandbox, built_bead, git, json};

use crate::commands::Held;

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

/// A machine with a tracker (prefix `ex`) and no remote: its checkout has no origin, and it runs
/// this build's bead, as [`tracker`]'s does. Its store opens with [`without_origin`].
pub(crate) fn tracker_without_remote() -> (Sandbox, Machine) {
    let sandbox = Sandbox::new(built_bead());
    let machine = Machine::new(&sandbox, "a").with_bead(built_bead());
    git(["remote", "remove", "origin"], Some(machine.checkout()));
    machine.bead(["init", "--prefix", "ex"]);
    (sandbox, machine)
}

/// Something a command or a write held, which counts the times it is freed.
#[derive(Debug)]
pub(crate) struct Probe(Arc<AtomicUsize>);

impl Held for Probe {}

impl Drop for Probe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// A probe, and the count of the times it has been freed.
pub(crate) fn probe() -> (Probe, Arc<AtomicUsize>) {
    let freed = Arc::new(AtomicUsize::new(0));
    (Probe(Arc::clone(&freed)), freed)
}
