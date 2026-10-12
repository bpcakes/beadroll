//! G52: a store directory bead cannot read, or a store path that is not a directory, is an `IO`
//! error. Reading it as "no journal here" made readers say `NOT_INITIALIZED` ("run bead init"),
//! `info` exit 0 with no issues, and an unreadable `claims/` turn a claimed issue ready. These tests
//! skip as root, whom directory modes do not stop, by returning at once.

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use beadroll_test_support::{Json, Machine, Run, parse_json};

/// Whether the test runs as root, who ignores
/// directory modes.
fn skipped_as_root() -> bool {
    let root = rustix::process::geteuid().is_root();
    if root {
        eprintln!("skipped: root ignores directory modes");
    }
    root
}

/// A failed JSON call's code and message; success returns `None` and stdout. A crash returns
/// `TRACEBACK`; an unstructured failure returns `UNSTRUCTURED`, both with stderr.
fn g52_error(run: &Run) -> (Option<String>, String) {
    if run.returncode() == 0 {
        return (None, run.stdout.clone());
    }
    if let Ok(document) = parse_json(&run.stderr) {
        let error = &document["error"];
        if let (Some(code), Some(message)) = (error["code"].as_str(), error["message"].as_str()) {
            return (Some(code.to_owned()), message.to_owned());
        }
    }
    let label = if run.crashed() {
        "TRACEBACK"
    } else {
        "UNSTRUCTURED"
    };
    (Some(label.to_owned()), run.stderr.clone())
}

/// `_g52_own_journal`: this machine's own journal directory, `events/<the id in replica.json>`.
fn own_journal(machine: &Machine) -> PathBuf {
    let replica = std::fs::read_to_string(machine.home().join("replica.json")).unwrap();
    let replica = parse_json(&replica).unwrap();
    let id = replica.get("id").and_then(Json::as_str).expect("an id");
    machine.store().join("events").join(id)
}

/// The id of the issue a `--json` result is: `result["id"]`.
fn id(result: Option<Json>) -> String {
    let result = result.expect("bead printed a result");
    result["id"].as_str().expect("an id").to_owned()
}

/// Gives `path` the requested mode until the guard drops, then restores its old mode before
/// the sandbox is removed.
struct Unreadable {
    path: PathBuf,
    old: u32,
}

impl Unreadable {
    fn new(path: &Path, mode: u32) -> Unreadable {
        let old = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        std::fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
        Unreadable {
            path: path.to_owned(),
            old,
        }
    }
}

impl Drop for Unreadable {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.path, Permissions::from_mode(self.old));
    }
}

mod g52_unreadable_store_is_io {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;

    use beadroll_test_support::{Machine, parse_json};

    use super::{Unreadable, g52_error, id, own_journal, skipped_as_root};

    /// G52: an unreadable journal directory makes `show` report IO naming the directory, not a
    /// missing tracker.
    #[test]
    fn g52_unlistable_own_journal_is_io_not_no_tracker() {
        if skipped_as_root() {
            return;
        }
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "g52-a");
        a.bead(["init", "--prefix", "ex"]);
        let key = id(a.bead(["create", "x"]));
        let journal = own_journal(&a);
        let _mode = Unreadable::new(&journal, 0o000);
        let (code, message) = g52_error(&a.run(["show", &key, "--json"]));
        assert_eq!(code.as_deref(), Some("IO"), "{message}");
        let journal = journal.to_str().unwrap();
        assert!(message.contains(journal), "{journal} not in {message}");
    }

    /// G52: with `claims/` unreadable, `ready` answers `IO`, instead of showing the claimed issue
    /// open and listing it (exit 0).
    #[test]
    fn g52_unlistable_claims_do_not_turn_a_claimed_issue_ready() {
        if skipped_as_root() {
            return;
        }
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "g52-a");
        a.bead(["init", "--prefix", "ex"]);
        let key = id(a.bead(["create", "x"]));
        a.bead(["update", &key, "--claim"]);
        assert_eq!(a.show(&key)["status"].as_str(), Some("in_progress"));
        let _mode = Unreadable::new(&own_journal(&a).join("claims"), 0o000);
        let (code, message) = g52_error(&a.run(["ready", "--json"]));
        assert_eq!(
            code.as_deref(),
            Some("IO"),
            "ready answered from a journal it could not read: {message}"
        );
    }

    /// G52: a create while the own journal cannot be listed is refused with `IO`, not acknowledged
    /// and then missing afterwards (it started a second chain, reported as a copied machine
    /// identity).
    #[test]
    fn g52_create_into_an_unlistable_journal_is_refused_not_lost() {
        if skipped_as_root() {
            return;
        }
        let sandbox = crate::sandbox();
        let (a, b) = (
            Machine::new(&sandbox, "g52-a"),
            Machine::new(&sandbox, "g52-b"),
        );
        a.bead(["init", "--prefix", "ex"]);
        a.bead(["sync"]);
        b.bead(["sync"]);
        b.bead(["create", "first"]);
        // writable and searchable, not listable
        let _mode = Unreadable::new(&own_journal(&b), 0o300);
        let (code, message) = g52_error(&b.run(["create", "second", "--json"]));
        if code.is_none() {
            // acknowledged: it must then exist
            let key = parse_json(&message).unwrap()["id"]
                .as_str()
                .expect("an id")
                .to_owned();
            std::fs::set_permissions(own_journal(&b), Permissions::from_mode(0o700)).unwrap();
            let shown = b.run(["show", &key, "--json"]);
            assert_eq!(
                shown.returncode(),
                0,
                "create returned {key} but it is gone: {}",
                shown.stderr.trim()
            );
        }
        assert_eq!(code.as_deref(), Some("IO"), "{message}");
    }

    /// G52: `BEAD_HOME` naming a regular file is `IO` for `show` and `info`, as it is for `create`
    /// on the same store, not `NOT_INITIALIZED` for `show` and exit 0 with no issues for `info`.
    #[test]
    fn g52_store_that_is_not_a_directory_is_io() {
        if skipped_as_root() {
            return;
        }
        let sandbox = crate::sandbox();
        let mut a = Machine::new(&sandbox, "g52-a");
        a.bead(["init", "--prefix", "ex"]);
        let key = id(a.bead(["create", "x"]));
        let not_a_dir = sandbox.root().join("home-file");
        std::fs::write(&not_a_dir, "not a directory\n").unwrap();
        a.extra
            .insert("BEAD_HOME".into(), not_a_dir.into_os_string());
        for args in [vec!["show", key.as_str()], vec!["info"]] {
            let (code, message) = g52_error(&a.run(args.iter().chain(&["--json"])));
            assert_eq!(code.as_deref(), Some("IO"), "{}: {message}", args[0]);
        }
    }

    /// G52: an inaccessible workspace store makes both `show` and `info` report IO. Neither may
    /// treat EACCES as an absent tracker. Skip under root, where chmod does not enforce this
    /// denial.
    #[test]
    fn g52_unenterable_store_is_io_not_no_tracker() {
        if skipped_as_root() {
            return;
        }
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "g52-a");
        a.bead(["init", "--prefix", "ex"]);
        let key = id(a.bead(["create", "x"]));
        // its mode comes back before the sandbox goes, which could not remove it otherwise
        let _mode = Unreadable::new(&a.store(), 0o000);
        for args in [vec!["show", key.as_str()], vec!["info"]] {
            let (code, message) = g52_error(&a.run(args.iter().chain(&["--json"])));
            assert_eq!(code.as_deref(), Some("IO"), "{}: {message}", args[0]);
        }
    }
}
