//! Maintenance of the remote, after or during the machines' operations, leaves convergence and
//! ownership whole.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use beadroll_test_support::{Machine, Sandbox, git};

use super::{Outcome, converge_and_compare, mixed_ops};

/// The class's `setUp`: machines a, b and c, a's tracker initialised (prefix `gc`) and published
/// with five issues, and every machine synced; with the issues' keys.
fn setup(sandbox: &Sandbox) -> ([Machine; 3], Vec<String>) {
    let machines = ["a", "b", "c"].map(|name| Machine::new(sandbox, name));
    let a = &machines[0];
    a.bead(["init", "--prefix", "gc"]);
    a.bead(["sync"]);
    let keys = (0..5)
        .map(|i| {
            let created = a.bead(["create", &format!("t{i}")]).expect("an issue");
            created["id"].as_str().expect("an id").to_owned()
        })
        .collect();
    for machine in &machines {
        machine.bead(["sync"]);
    }
    (machines, keys)
}

/// The class's `check`: nothing unexpected failed, the machines converge, and the confirmed claims
/// a lists are the owner model's.
fn check(machines: &[Machine; 3], origin: &Path, (owner, unexpected): Outcome) {
    assert!(unexpected.is_empty(), "{unexpected:?}");
    let all: Vec<&Machine> = machines.iter().collect();
    let claims = converge_and_compare(&all, origin);
    let mut seen: Vec<(String, String)> = claims["a"]
        .iter()
        .filter(|(_, _, state)| state == "confirmed")
        .map(|(id, owner, _)| {
            let (_, holder) = owner.split_once(':').expect("an owner agent:thread");
            let name = holder.split("-thread").next().expect("a machine name");
            (id.clone(), name.to_owned())
        })
        .collect();
    seen.sort();
    let expected: Vec<(String, String)> = owner.into_iter().collect();
    assert_eq!(seen, expected);
}

/// `_v8_wait_for_remote_gc`, also as a cleanup: a detached `gc --auto` may still be running on the
/// remote; let it finish (for up to 20 seconds) before the sandbox goes.
struct RemoteGc(PathBuf);

impl RemoteGc {
    fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.0.join("gc.pid").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for RemoteGc {
    fn drop(&mut self) {
        self.wait();
    }
}

/// A failed maintenance command: its arguments and what it printed on standard error.
type Failure = (Vec<&'static str>, String);

/// `repack -adf`, `prune-packed` and `gc --auto` looping on the remote on a thread of their own
/// until stopped, which dropping the loop does too: the Python test's `finally`.
struct Maintenance {
    stop: Arc<AtomicBool>,
    failures: Arc<Mutex<Vec<Failure>>>,
    worker: Option<JoinHandle<()>>,
}

impl Maintenance {
    fn start(remote: &Path) -> Maintenance {
        let stop = Arc::new(AtomicBool::new(false));
        let failures: Arc<Mutex<Vec<Failure>>> = Arc::default();
        let (looping, failed, remote) =
            (Arc::clone(&stop), Arc::clone(&failures), remote.to_owned());
        let worker = std::thread::spawn(move || {
            let commands: [&[&'static str]; 3] = [
                &["repack", "-adf", "-q"],
                &["prune-packed"],
                &["gc", "--auto", "--quiet"],
            ];
            while !looping.load(Ordering::SeqCst) {
                for cmd in commands {
                    let out = Command::new("git")
                        .arg("--git-dir")
                        .arg(&remote)
                        .args(cmd)
                        .output()
                        .expect("git starts");
                    if !out.status.success() {
                        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                        failed.lock().unwrap().push((cmd.to_vec(), stderr));
                    }
                }
            }
        });
        Maintenance {
            stop,
            failures,
            worker: Some(worker),
        }
    }

    /// Stops the loop and waits for it: the commands that failed.
    fn stop(&mut self) -> Vec<Failure> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("the maintenance loop");
        }
        self.failures.lock().unwrap().clone()
    }
}

impl Drop for Maintenance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Verified (V8): remote maintenance run after every remote operation (not concurrently) leaves
/// convergence and ownership intact. Here `git gc --prune=now --aggressive` runs on the bare remote
/// after every sync/claim/release of 60 mixed operations on 3 machines. The review's soak (300 ops,
/// 3 machines, modes gc --prune=now --aggressive, gc --auto with gc.auto=1, repack -adf +
/// prune-packed) measured, in each mode: exports identical, claims identical, owners equal to the
/// model (16 accepted claims), integrity [], waiting 0, pending [], fsck 0, 0 unexpected errors
/// over 200/200/400 maintenance runs; sync mean/p95 453/645 ms without gc vs 498/789 ms with the
/// aggressive gc.
#[test]
fn gc_prune_now_aggressive_after_every_remote_operation() {
    let sandbox = crate::sandbox();
    let (machines, keys) = setup(&sandbox);
    let origin = sandbox.origin();
    let mut gc = || {
        let out = Command::new("git")
            .arg("--git-dir")
            .arg(origin)
            .args(["gc", "--prune=now", "--aggressive", "--quiet"])
            .output()
            .expect("git starts");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let all: Vec<&Machine> = machines.iter().collect();
    let outcome = mixed_ops(&all, &keys, 60, 7, Some(&mut gc));
    check(&machines, origin, outcome);
}

/// Verified (V8): `git repack -adf` + `prune-packed` and `gc --auto` with gc.auto=1 looping on the
/// remote CONCURRENTLY with the machines' pushes and fetches (60 mixed operations here) cause no
/// maintenance error, no unexpected bead error and no divergence. The review measured, over 300
/// ops: repack 4740 maintenance runs and auto1 8529 runs, 0 errors, owner model 17/17, fsck 0;
/// gc_race with 3 machines publishing in parallel saw 1154 repack runs during 150 commands, 0
/// errors, identical exports and claims. (Concurrent `gc --prune=now` corrupted the remote in 4 of
/// 4 runs, finding ENV-4, and is not exercised here.)
#[test]
fn repack_and_auto_gc_running_concurrently_with_pushes_and_fetches() {
    let sandbox = crate::sandbox();
    let (machines, keys) = setup(&sandbox);
    let origin = sandbox.origin().to_path_buf();
    let config = [
        "--git-dir",
        origin.to_str().expect("a UTF-8 path"),
        "config",
        "gc.auto",
        "1",
    ];
    git(config, None);
    let remote_gc = RemoteGc(origin.clone());
    let mut maintenance = Maintenance::start(&origin);
    let all: Vec<&Machine> = machines.iter().collect();
    let outcome = mixed_ops(&all, &keys, 60, 8, None);
    let failures = maintenance.stop();
    assert!(failures.is_empty(), "{failures:?}");
    remote_gc.wait();
    check(&machines, &origin, outcome);
}
