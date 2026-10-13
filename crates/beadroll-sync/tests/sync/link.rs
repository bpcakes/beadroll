//! What a link keeps of git (`crates/beadroll-git/AGENTS.md`): the long-lived gits it asks end with
//! it, killed and reaped, on every path out, an error's included.

use std::process::Command;

use beadroll_store::Endpoint;
use beadroll_sync::synchronise;
use beadroll_test_support::Machine;

use crate::support::{sandbox, workspace};

/// This process's children, live or not yet reaped, but for the `ps` that lists them: its rows
/// whose parent is this process.
fn children() -> Vec<String> {
    let ps = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,stat=,command="])
        .output()
        .expect("ps runs");
    let me = std::process::id().to_string();
    String::from_utf8_lossy(&ps.stdout)
        .lines()
        .filter(|row| {
            let fields: Vec<&str> = row.split_whitespace().collect();
            fields.get(1) == Some(&me.as_str()) && fields.get(3) != Some(&"ps")
        })
        .map(str::to_owned)
        .collect()
}

/// How many descriptors this process has open.
fn open_descriptors() -> Option<usize> {
    std::fs::read_dir("/dev/fd").ok().map(Iterator::count)
}

/// Syncs through links that went through, and through one an unreachable remote ended, leave this
/// process no child, live or zombie, and no descriptor more than before.
#[test]
fn the_gits_a_link_keeps_end_with_it() {
    let sandbox = sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    a.bead(["sync"]);
    let ws = workspace(&a, sandbox.origin(), &[]);
    let origin = Endpoint::at(sandbox.origin());
    synchronise(&ws, &origin).expect("a first sync");
    let before = open_descriptors();
    for _ in 0..3 {
        synchronise(&ws, &origin).expect("a sync");
    }
    let nowhere = sandbox.root().join("nowhere.git");
    let error = synchronise(&ws, &Endpoint::at(&nowhere)).expect_err("no remote there");
    assert_eq!(error.code().as_str(), "REMOTE_UNREACHABLE", "{error}");
    assert_eq!(children(), Vec::<String>::new());
    assert_eq!(open_descriptors(), before);
}
