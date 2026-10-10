//! `ProtocolReview4`: a release confirmed here but cut off before its pending twin was removed is
//! recognised as done when it is run again (the fourth review, Q03).

use beadroll_core::code::Code;
use beadroll_store::{claim_files, detect_actor, find_workspace, pending_files};
use beadroll_test_support::Machine;

use crate::support::{release_command, sandbox, settings, text};

/// Q03: a final attempt plus its pending twin must be recognised as accepted.
#[test]
fn q03_crash_after_confirmation_before_cleanup_is_idempotent() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "q03-a");
    a.bead(["init", "--prefix", "ex"]);
    let key = text(&a.bead(["create", "release once"]).unwrap()["id"]);
    a.bead(["claim", &key]);

    // the release counts here, and fails before its pending twin is removed
    let settings = settings(
        &a,
        &[("BEAD_FAILPOINTS", "ownership.after-confirm=error(IO)")],
    );
    let ws = find_workspace(&settings).unwrap();
    let actor = detect_actor(&settings, None, &|| None);
    let error = release_command(&ws, &actor, &key).unwrap_err();
    assert_eq!(error.code(), Code::Io, "{error}");
    assert_eq!(
        error.to_string(),
        "IO injected by the ownership.after-confirm failpoint"
    );
    assert_eq!(pending_files(&ws).len(), 1);
    assert_eq!(claim_files(&ws).unwrap().len(), 2);

    assert_eq!(a.show(&key)["status"].as_str(), Some("open"));
    let retry = a.run(["release", &key, "--json"]);
    assert_eq!(retry.returncode(), 0, "{}", retry.stderr);
    assert_eq!(a.show(&key)["status"].as_str(), Some("open"));
    assert_eq!(pending_files(&ws), Vec::<std::path::PathBuf>::new());
    let log = a.bead(["log", &key, "--limit", "0"]).unwrap();
    let releases = log
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["op"].as_str() == Some("release"))
        .count();
    assert_eq!(
        releases, 1,
        "recovery retained or appended a second release"
    );
}
