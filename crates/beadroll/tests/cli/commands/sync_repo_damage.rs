//! A sync repository containing empty loose objects must be rebuilt instead of reporting the
//! healthy remote as unreachable. It is a cache of the remote and `events/`, so both sync and
//! claim set it aside and rebuild once per command. The classifier checks are in beadroll-git's
//! `commands::sync_repo_damage`.

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use beadroll_test_support::{Json, Machine, Sandbox, git, parse_json};

/// `setUp`: machine a's tracker holds issue x and is published; machine b has synced it. Also x's
/// key.
fn set_up(sandbox: &Sandbox) -> (Machine, Machine, String) {
    let a = Machine::new(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "x"]).unwrap();
    let key = created["id"].as_str().unwrap().to_owned();
    a.bead(["sync"]);
    let b = Machine::new(sandbox, "b");
    b.bead(["sync"]);
    (a, b, key)
}

/// Empties the loose object of the commit machine a's sync.git remembers as the remote's head:
/// fetch cannot even negotiate ('cannot read existing object info').
fn damage(a: &Machine) -> PathBuf {
    let sync_git = a.store().join("sync.git");
    let head = git(
        [
            "--git-dir",
            sync_git.to_str().unwrap(),
            "rev-parse",
            "refs/beads/remote",
        ],
        None,
    );
    let head = head.trim();
    let loose = sync_git.join("objects").join(&head[..2]).join(&head[2..]);
    assert!(
        loose.exists(),
        "the head commit is a loose object in a fresh sync.git"
    );
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(&loose, b"").unwrap();
    sync_git
}

#[test]
fn sync_rebuilds_a_damaged_sync_repo_once_and_info_lists_the_copy() {
    let sandbox = crate::sandbox();
    let (a, b, _) = set_up(&sandbox);
    b.bead(["create", "from b"]);
    b.bead(["sync"]);
    let sync_git = damage(&a);
    let synced = a.run(["sync", "--json"]);
    assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
    let result = parse_json(&synced.stdout).unwrap();
    let rebuilt = result.get("sync_repo_rebuilt").and_then(Json::as_str);
    let aside = format!("{}.damaged-", sync_git.to_str().unwrap());
    assert!(
        rebuilt.unwrap_or_default().starts_with(&aside),
        "{result:?}"
    );
    assert_eq!(result["pulled"].as_i64(), Some(1));
    let listed = a.bead(["list"]).unwrap();
    let mut titles: Vec<&str> = listed["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect();
    titles.sort_unstable();
    assert_eq!(titles, ["from b", "x"]);
    assert_eq!(
        a.bead(["info"]).unwrap()["integrity"],
        Json::Array(Vec::new())
    );
    let info = a.bead(["info"]).unwrap();
    let copies = info["damaged_sync_repos"].as_array().unwrap();
    assert_eq!(copies.len(), 1);
    assert!(
        a.store()
            .join(copies[0].as_str().unwrap())
            .join("HEAD")
            .exists(),
        "the damaged repository is kept for a look"
    );
    // the rebuilt one is a normal sync.git
    assert_eq!(a.run(["sync", "--json"]).returncode(), 0);
    let again = parse_json(&a.run(["sync", "--json"]).stdout).unwrap();
    assert!(again.get("sync_repo_rebuilt").is_none(), "{again:?}");
}

/// A claim rebuilds a damaged sync repository and succeeds through the replacement; `info` lists
/// the one copy set aside.
#[test]
fn a_claim_rebuilds_too_and_damage_twice_in_one_command_is_reported() {
    let sandbox = crate::sandbox();
    let (a, b, key) = set_up(&sandbox);
    damage(&a);
    let claimed = a.run(["claim", &key, "--json"]);
    assert_eq!(claimed.returncode(), 0, "{}", claimed.stderr);
    b.bead(["sync"]);
    let shown = b.show(&key);
    let owner = shown["claim"]["owner"].as_str().unwrap_or_default();
    assert!(
        owner.contains("codex:"),
        "the claim went through the rebuilt sync.git: {shown:?}"
    );
    let info = a.bead(["info"]).unwrap();
    assert_eq!(
        info["damaged_sync_repos"].as_array().map(<[Json]>::len),
        Some(1)
    );
}
