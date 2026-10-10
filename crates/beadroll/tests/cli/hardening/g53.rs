//! G53: `replica_id` mapped every failure to read `replica.json`, a permission error included, to
//! `IDENTITY` "... is unreadable; restore it, or move it aside to mint a new machine identity".
//! Following that advice for what is only a mode problem changes every actor label on the machine,
//! so its agents can no longer close or release the claims they hold. The class is skipped as root,
//! who reads a mode-0 file; its tests then return at once.

use beadroll_test_support::{Run, parse_json};

/// `_g53_error`: the code and message of a failed `--json` call; `TRACEBACK` and the standard
/// error when bead crashed (a Python traceback, a Rust panic or a fatal signal), and `None` and
/// the standard error when it holds no structured error.
fn g53_error(run: &Run) -> (Option<String>, String) {
    if run.crashed() {
        return (Some("TRACEBACK".to_owned()), run.stderr.clone());
    }
    if let Ok(document) = parse_json(&run.stderr) {
        let error = &document["error"];
        if let (Some(code), Some(message)) = (error["code"].as_str(), error["message"].as_str()) {
            return (Some(code.to_owned()), message.to_owned());
        }
    }
    (None, run.stderr.clone())
}

mod g53_unreadable_identity_is_io {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use beadroll_test_support::{Machine, Sandbox};

    use super::g53_error;

    /// `setUp`'s skip: whether the test runs as root, who reads a mode-0 file.
    fn skipped_as_root() -> bool {
        let root = rustix::process::geteuid().is_root();
        if root {
            eprintln!("skipped: root reads a mode-000 file");
        }
        root
    }

    /// `setUp`: machine `a` with a tracker (prefix `gi`) and one issue, which it has claimed; the
    /// issue's key and the path of the machine's `replica.json`.
    fn set_up(sandbox: &Sandbox) -> (Machine, String, PathBuf) {
        let a = Machine::new(sandbox, "a");
        a.bead(["init", "--prefix", "gi"]);
        let created = a.bead(["create", "x"]).expect("bead printed a result");
        let key = created["id"].as_str().expect("an id").to_owned();
        a.bead(["claim", &key]);
        let replica = a.home().join("replica.json");
        (a, key, replica)
    }

    /// G53: a permission error on `replica.json` is `IO`, and the advice is not to mint a new
    /// identity (which would orphan this machine's claims): fixing the mode is the whole cure.
    #[test]
    fn g53_unreadable_identity_is_io_and_does_not_advise_a_new_identity() {
        if skipped_as_root() {
            return;
        }
        let sandbox = crate::sandbox();
        let (a, key, replica) = set_up(&sandbox);
        std::fs::set_permissions(&replica, Permissions::from_mode(0o000)).unwrap();
        let closed = a.run(["close", &key, "--json"]);
        // The `finally`: should starting bead panic, the sandbox goes with the file all the same.
        std::fs::set_permissions(&replica, Permissions::from_mode(0o600)).unwrap();
        assert_eq!(closed.returncode(), 1, "{}", closed.stderr);
        let (code, message) = g53_error(&closed);
        assert_eq!(
            (code.as_deref(), message.contains("mint a new")),
            (Some("IO"), false),
            "a permission problem is reported as a damaged identity: {}",
            closed.stderr.trim()
        );
        let again = a.run(["close", &key, "--json"]);
        assert_eq!(again.returncode(), 0, "{}", again.stderr);
        assert_eq!(a.show(&key)["status"].as_str(), Some("closed"));
    }

    /// G53: guard for the fix (it passed before it too): a `replica.json` that is not UTF-8 is
    /// still a damaged identity (F13), a structured `IDENTITY` error, not a crash.
    #[test]
    fn g53_guard_undecodable_identity_stays_a_structured_identity_error() {
        if skipped_as_root() {
            return;
        }
        let sandbox = crate::sandbox();
        let (a, key, replica) = set_up(&sandbox);
        std::fs::write(&replica, b"{\"id\": \"\xff\xfe\"}\n").unwrap();
        let closed = a.run(["close", &key, "--json"]);
        assert_eq!(
            g53_error(&closed).0.as_deref(),
            Some("IDENTITY"),
            "{}",
            closed.stderr
        );
    }
}
