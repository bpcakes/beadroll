//! A prune of a machine's own sync.git while it syncs is local damage that the next commands
//! repair.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use beadroll_test_support::{Machine, code};

use super::converge_and_compare;

/// Loops `git gc --prune=now` on `sync_git` in a separate thread until stopped or dropped.
struct Prune {
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Prune {
    fn start(sync_git: &std::path::Path) -> Prune {
        let stop = Arc::new(AtomicBool::new(false));
        let (looping, sync_git) = (Arc::clone(&stop), sync_git.to_owned());
        let worker = std::thread::spawn(move || {
            while !looping.load(Ordering::SeqCst) {
                let _ = Command::new("git")
                    .arg("--git-dir")
                    .arg(&sync_git)
                    .args(["gc", "--prune=now", "--quiet"])
                    .stdin(Stdio::null())
                    .output();
            }
        });
        Prune {
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for Prune {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Verified (V8): `git gc --prune=now` looping on a machine's own private sync.git during its syncs
/// causes only transient errors; nothing is left pending and the next syncs converge. The review's
/// soak (776 prune runs during 200 ops) saw 89 GIT_ERROR ('git mktree: fatal: entry ... is
/// unavailable') and 2 REMOTE_UNREACHABLE, all transient; final exports and claims identical,
/// pending [], integrity [], remote fsck 0. Here: 30 create+sync rounds on a while the prune loops,
/// then b sees all 30 issues. A prune that removes what a fetch brought before bead reads it is
/// local damage too: the command makes sync.git again, and a second hit in it is
/// SYNC_REPO_DAMAGED. That is the likely cause of the MANIFEST_MISSING this test answered now and
/// then on macOS CI, which no soak elsewhere reproduced (roll-dih8.30); `v8_prune_after_a_fetch`
/// makes its cases deterministic. A prune during the fetch itself, which deletes the fetch's
/// temporary pack, is local damage too, found by the fetch.
#[test]
fn prune_now_on_the_private_sync_repo_during_syncs_is_transient() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let b = Machine::new(&sandbox, "b");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["sync"]);
    b.bead(["sync"]);
    let sync_git = a.store().join("sync.git");
    assert!(sync_git.is_dir(), "{}", sync_git.display());
    let mut codes = Vec::new();
    {
        let _prune = Prune::start(&sync_git);
        for i in 0..30 {
            a.bead(["create", &format!("t{i}")]);
            let run = a.run(["sync", "--json"]);
            if run.returncode() != 0 {
                codes.push(code(&run));
            }
        }
    }
    // a prune under a running command is local damage: bead rebuilds sync.git once per command,
    // and a second hit in the same command is SYNC_REPO_DAMAGED; the next command rebuilds and
    // converges
    let transient = ["GIT_ERROR", "REMOTE_UNREACHABLE", "SYNC_REPO_DAMAGED"];
    assert!(
        codes.iter().all(|code| transient.contains(&code.as_str())),
        "{codes:?}"
    );
    converge_and_compare(&[&a, &b], sandbox.origin());
    let listed = b.bead(["list", "--all", "--limit", "0"]).expect("a list");
    let issues = listed["issues"].as_array().expect("a list of issues");
    assert_eq!(issues.len(), 30);
}
