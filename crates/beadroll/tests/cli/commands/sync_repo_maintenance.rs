//! `SyncRepoMaintenance`: git's own maintenance is off in sync.git, fetches stay packed, and bead
//! repacks under the store lock when objects pile up.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::Arc;

use beadroll_git::SYNC_REPO_CONFIG;
use beadroll_store::{Endpoint, Settings, find_workspace};
use beadroll_sync::Link;
use beadroll_test_support::{Json, Machine, Sandbox, git, parse_json};

/// `setUp`: machine a's tracker holds issue x (its key returned) and is published; machine b has
/// synced it.
fn set_up(sandbox: &Sandbox) -> (Machine, String, Machine) {
    let a = Machine::new(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "x"]).unwrap();
    let key = created["id"].as_str().unwrap().to_owned();
    a.bead(["sync"]);
    let b = Machine::new(sandbox, "b");
    b.bead(["sync"]);
    (a, key, b)
}

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
    let (a, _, b) = set_up(&sandbox);

    // The link machine a's bead builds, in its checkout and environment.
    let vars: BTreeMap<OsString, OsString> = a.env(None, "codex").into_iter().collect();
    let user_home = std::env::var_os("HOME").unwrap_or_default();
    let settings = Arc::new(
        Settings::from_process(
            vars,
            Some(a.checkout().to_path_buf()),
            user_home,
            std::env::temp_dir(),
            &|_| None,
        )
        .unwrap(),
    );
    let ws = find_workspace(&settings).unwrap();
    let link = Link::new(&ws, &Endpoint::at(sandbox.origin())).unwrap();
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

#[test]
fn repacks_under_the_lock_when_loose_objects_pile_up() {
    let sandbox = crate::sandbox();
    let (a, key, b) = set_up(&sandbox);
    for n in 0..3 {
        a.bead(["create", &format!("t{n}")]);
        a.bead(["sync"]);
    }
    let (loose_before, _) = counts(&a);
    assert!(loose_before > 3, "{loose_before}");
    let synced = a
        .cmd(["sync", "--json"])
        .env("BEAD_REPACK_LOOSE", "3")
        .run();
    let result = parse_json(&synced.stdout).unwrap();
    assert!(
        result.get("repacked").is_some_and(Json::truthy),
        "{result:?}"
    );
    let (loose, packs) = counts(&a);
    assert!(loose <= 3, "packed copies are gone");
    assert_eq!(packs, 1, "one pack");
    let sync_git = a.store().join("sync.git");
    let fsck = git(
        [
            "--git-dir",
            sync_git.to_str().unwrap(),
            "fsck",
            "--full",
            "--no-dangling",
        ],
        None,
    );
    assert_eq!(fsck.trim(), "");
    // nothing piled up since
    let again = parse_json(&a.run(["sync", "--json"]).stdout).unwrap();
    assert!(again.get("repacked").is_none(), "{again:?}");
    a.bead(["claim", &key]);
    b.bead(["sync"]);
    let owner = b.show(&key)["claim"]["owner"].clone();
    assert!(owner.as_str().unwrap().contains("codex:"), "{owner:?}");
}
