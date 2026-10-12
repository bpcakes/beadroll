//! V6: malformed or inaccessible store paths produce structured errors, never crashes. Cases
//! include a lock or segment replaced by a directory, pending replaced by a file and read-only
//! claims. Skip under root, where chmod cannot enforce denial.

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use beadroll_test_support::Machine;

use super::{error_code, id, replica_id, unprivileged};

/// What undoes a damage.
type Repair = Box<dyn FnOnce()>;

/// `as_dir`: `path` set aside as `PATH.saved` and a folder made in its place; the repair removes
/// the folder and puts the file back.
fn as_dir(path: &Path) -> Repair {
    let mut name = path.file_name().expect("a name").to_owned();
    name.push(".saved");
    let saved = path.with_file_name(name);
    std::fs::rename(path, &saved).unwrap();
    std::fs::create_dir(path).unwrap();
    let path = path.to_owned();
    Box::new(move || {
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&saved, &path).unwrap();
    })
}

/// `chmod`: `path` given `mode`; the repair gives it `back`.
fn chmod(path: &Path, mode: u32, back: u32) -> Repair {
    std::fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
    let path = path.to_owned();
    Box::new(move || std::fs::set_permissions(&path, Permissions::from_mode(back)).unwrap())
}

/// `pending_file`: the store's `pending` folder, made if it was not there, replaced by a file; the
/// repair removes the file.
fn pending_file(store: &Path) -> Repair {
    let pending = store.join("pending");
    if let Err(error) = std::fs::create_dir(&pending) {
        assert!(pending.is_dir(), "{error}");
    }
    std::fs::remove_dir(&pending).unwrap();
    std::fs::write(&pending, "x").unwrap();
    Box::new(move || std::fs::remove_file(&pending).unwrap())
}

/// Verified (V6): damage to the local store is answered with a structured error code, never a
/// traceback. The review ran 16 damages x 5 commands (show, create, claim, sync, comments add) =
/// 80 runs with 0 tracebacks; lock dir / lock 000 gave IO for writers with show ok, segment 000
/// or a directory gave IO everywhere, pending/ as a file gave claim IO, and the replica's claims
/// dir at 0500 gave claim IO after the push. This test covers 6 of those damages x the 5 commands
/// and accepts only IO, GIT_ERROR or REMOTE_UNREACHABLE for a failure.
#[test]
fn store_damage_is_answered_with_a_structured_code() {
    if !unprivileged() {
        return;
    }
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = id(a.bead(["create", "x"]));
    a.bead(["sync"]);
    let store = a.store();
    let replica = replica_id(&a);
    let replica = replica.as_str().expect("a replica id");
    let seg = store.join("events").join(replica).join("000001.jsonl");
    let claims = store.join("events").join(replica).join("claims");
    if let Err(error) = std::fs::create_dir(&claims) {
        assert!(claims.is_dir(), "{error}");
    }
    let damage = |name: &str| -> Repair {
        match name {
            "lock dir" => as_dir(&store.join("lock")),
            "lock 000" => chmod(&store.join("lock"), 0o000, 0o600),
            "segment 000" => chmod(&seg, 0o000, 0o600),
            "segment dir" => as_dir(&seg),
            "pending file" => pending_file(&store),
            "claims 0500" => chmod(&claims, 0o500, 0o700),
            other => panic!("no damage {other:?}"),
        }
    };
    let damages = [
        "lock dir",
        "lock 000",
        "segment 000",
        "segment dir",
        "pending file",
        "claims 0500",
    ];
    for name in damages {
        let commands: [&[&str]; 5] = [
            &["show", &key],
            &["create", "n"],
            &["claim", &key],
            &["sync"],
            &["comments", "add", &key, "c"],
        ];
        for argv in commands {
            let repair = damage(name);
            let proc = a.run(argv.iter().chain(&["--json"]));
            repair();
            a.run(["release", &key]);
            assert!(!proc.crashed(), "{name} {}: {}", argv[0], proc.stderr);
            if proc.returncode() != 0 {
                let code = error_code(&proc);
                assert!(
                    matches!(
                        code.as_deref(),
                        Some("IO" | "GIT_ERROR" | "REMOTE_UNREACHABLE")
                    ),
                    "{name} {}: {code:?}: {}",
                    argv[0],
                    proc.stderr
                );
            }
        }
    }
}
