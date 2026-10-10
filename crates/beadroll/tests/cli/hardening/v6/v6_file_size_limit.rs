//! `V6_FileSizeLimit`: a write past the file-size limit is `IO` (or, when git meets the limit
//! first, `GIT_ERROR` naming the SIGXFSZ that killed it), and the next write and sync are clean.
//! The limit is the Python test's own, `ulimit -f 200` in `/bin/sh`, in whatever blocks that shell
//! counts; without a `/bin/sh` the class skips, as the Python one does.

use std::collections::BTreeSet;

use beadroll_test_support::{Json, Machine};

use super::{error_code, limited, member, with_sh};

/// Verified (V6): EFBIG from a file-size limit is IO, and the next write and sync are clean.
/// The review measured, under `ulimit -f 200`: create with a 300 KB --description-file IO
/// ('[Errno 27] File too large'), the next create exit 0; sync pulling another machine's 300 KB
/// segment IO, the next sync exit 0; integrity [] afterwards. Under `ulimit -f 1200` both
/// succeeded.
#[test]
fn file_size_limit_is_io_and_the_next_write_and_sync_are_clean() {
    if !with_sh() {
        return;
    }
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["sync"]);
    let other = Machine::new(&sandbox, "other");
    other.bead(["sync"]);
    let body = sandbox.root().join("big.txt");
    std::fs::write(&body, "z".repeat(300_000)).unwrap();
    let body = body.to_str().expect("a UTF-8 path");

    let created = limited(&a, "-f 200", &["create", "big", "--description-file", body]);
    assert!(!created.crashed(), "{}", created.stderr);
    assert_eq!(
        error_code(&created).as_deref(),
        Some("IO"),
        "{}",
        created.stderr
    );
    let after = a.run(["create", "after", "--json"]);
    assert_eq!(after.returncode(), 0, "{}", after.stderr);
    other.bead(["create", "growth", "--description-file", body]);
    other.bead(["sync"]);
    let synced = limited(&a, "-f 200", &["sync"]);
    assert!(!synced.crashed(), "{}", synced.stderr);
    // Adapted on closing G58: git's output for bead now goes through a file bead reads back, so the
    // limit can hit git first (GIT_ERROR naming the SIGXFSZ that killed it) before bead's own
    // segment write (IO).
    let code = error_code(&synced);
    assert!(
        matches!(code.as_deref(), Some("IO" | "GIT_ERROR")),
        "{code:?}: {}",
        synced.stderr
    );
    assert!(
        synced.stderr.contains("File too large") || synced.stderr.contains("SIGXFSZ"),
        "{}",
        synced.stderr
    );
    let resynced = a.run(["sync", "--json"]);
    assert_eq!(resynced.returncode(), 0, "{}", resynced.stderr);
    let info = a.bead(["info"]).expect("bead info prints a record");
    assert_eq!(member(&info, "integrity"), &Json::Array(Vec::new()));
    let listed = a.bead(["list"]).expect("bead list prints a record");
    let issues = member(&listed, "issues").as_array().expect("a list");
    let titles: BTreeSet<&str> = issues
        .iter()
        .map(|issue| member(issue, "title").as_str().expect("a title"))
        .collect();
    assert!(titles.contains("growth"), "{titles:?}");
}
