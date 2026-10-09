//! `SyncRepoMaintenance`: git's own maintenance is off in sync.git, fetches stay packed, and bead
//! repacks under the store lock when objects pile up.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::Arc;

use beadroll_git::{Env, SYNC_REPO_CONFIG};
use beadroll_store::{Settings, find_workspace};
use beadroll_sync::Link;
use beadroll_test_support::Machine;

/// The loose objects and packs of a machine's sync.git.
fn counts(machine: &Machine) -> (usize, usize) {
    let objects = machine.store().join("sync.git").join("objects");
    let mut loose = 0;
    for entry in std::fs::read_dir(&objects).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap().len() == 2 {
            loose += std::fs::read_dir(&path).unwrap().count();
        }
    }
    let packs = match std::fs::read_dir(objects.join("pack")) {
        Ok(entries) => entries
            .filter(|e| {
                let path = e.as_ref().unwrap().path();
                path.extension().is_some_and(|ext| ext == "pack")
            })
            .count(),
        Err(_) => 0,
    };
    (loose, packs)
}

#[test]
fn no_git_maintenance_in_sync_git_and_fetches_stay_packed() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    a.bead(["sync"]);
    let b = Machine::new(&sandbox, "b");
    b.bead(["sync"]);

    // The link machine a's bead builds, in its checkout and environment.
    let vars: BTreeMap<OsString, OsString> = a.env(None, "codex").into_iter().collect();
    let user_home = std::env::var_os("HOME").unwrap_or_default();
    let mut settings = Settings::from_process(
        vars.clone(),
        Some(a.checkout().to_path_buf()),
        user_home,
        std::env::temp_dir(),
        &|_| None,
    )
    .unwrap();
    settings.git = Env::new(vars, Some(a.checkout().to_path_buf()), std::env::temp_dir());
    let settings = Arc::new(settings);
    let ws = find_workspace(&settings).unwrap();
    let link = Link::new(&ws, sandbox.origin().as_os_str()).unwrap();
    for (key, value) in SYNC_REPO_CONFIG {
        let seen = link.env.git(["config", "--get", key]).string().unwrap();
        assert_eq!(seen, value, "{key}");
    }
    let (_, packs_before) = counts(&a);
    b.bead(["create", "from b"]);
    b.bead(["sync"]);
    a.bead(["sync"]);
    let (_, packs_after) = counts(&a);
    assert_eq!(
        packs_after,
        packs_before + 1,
        "what the fetch brought is one pack, not loose files"
    );
}
