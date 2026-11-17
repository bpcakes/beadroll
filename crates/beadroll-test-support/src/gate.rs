//! The `pause-until(PATH)` handshake (`crates/beadroll-store/src/failpoint.rs`): bead touches
//! `PATH.ready` on reaching the failpoint, then waits for the test to touch `PATH.go` after acting
//! at that boundary. This is how a test holds bead at a point; a fixed sleep never is.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::{Run, Sandbox};

/// How long a test waits for bead to reach a gate before it gives up on it.
const PATIENCE: Duration = Duration::from_secs(120);

/// A gate a `pause-until` failpoint stops at, named in the sandbox.
pub struct Gate {
    path: PathBuf,
}

impl Gate {
    /// The gate `name` in `sandbox`.
    pub fn new(sandbox: &Sandbox, name: &str) -> Gate {
        Gate {
            path: sandbox.root().join(name),
        }
    }

    /// The failpoint action that stops at this gate: `pause-until(PATH)`.
    pub fn action(&self) -> String {
        format!("pause-until({})", self.path.display())
    }

    /// The `BEAD_FAILPOINTS` spec that stops at this gate at `point`, which may carry a selector
    /// (`journal.after-enumerate@always`).
    pub fn at(&self, point: &str) -> String {
        format!("{point}={}", self.action())
    }

    /// The file bead touches once it has stopped at this gate.
    pub fn ready(&self) -> PathBuf {
        self.with(".ready")
    }

    /// Once bead has reached the gate, runs `step` and then opens the gate, even if `step` fails:
    /// what `step` came to, or `None` when bead finished (`finished`) without reaching it. The gate
    /// is opened however this ends, so that bead never waits out its limit for a test that gave
    /// up; once open it stays open, and bead passes any later hit of the point at once.
    pub fn meet<T>(&self, finished: &dyn Fn() -> bool, step: impl FnOnce() -> T) -> Option<T> {
        let ready = self.ready();
        let _open = Open(self.with(".go"));
        let deadline = Instant::now() + PATIENCE;
        while !ready.exists() {
            if finished() || Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Some(step())
    }

    /// Opens the gate now: bead, waiting at it or yet to reach it, passes it at once.
    pub fn open(&self) {
        std::fs::write(self.with(".go"), b"").expect("the gate's go file");
    }

    /// The gate's path with `suffix` added to its name.
    fn with(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    }
}

/// Touches its file when dropped: the gate opened, whatever the test came to. It ignores a failed
/// write, since it runs while a failed test unwinds too.
struct Open(PathBuf);

impl Drop for Open {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, b"");
    }
}

/// Runs `bead` (a run of it, to the end) in a thread of its own while `test` meets it at its gates,
/// told whether it has finished: what each came to.
pub fn beside<T>(
    bead: impl FnOnce() -> Run + Send,
    test: impl FnOnce(&dyn Fn() -> bool) -> T,
) -> (Run, T) {
    std::thread::scope(|scope| {
        let running = scope.spawn(bead);
        let met = test(&|| running.is_finished());
        let run = running
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (run, met)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sandbox with nothing to run: any file stands in for bead.
    fn sandbox() -> Sandbox {
        let this = std::env::current_exe().expect("the running test's own executable");
        Sandbox::selecting(this, None)
    }

    #[test]
    fn the_spec_names_the_point_and_the_gate() {
        let sandbox = sandbox();
        let gate = Gate::new(&sandbox, "g");
        let path = sandbox.root().join("g");
        assert_eq!(
            gate.at("publish.before@always"),
            format!("publish.before@always=pause-until({})", path.display())
        );
        assert_eq!(gate.ready(), sandbox.root().join("g.ready"));
    }

    /// A paused process stands in for bead: the step runs while it waits, and it goes on after.
    #[test]
    fn a_meeting_runs_the_step_while_the_process_waits_and_then_lets_it_go() {
        let sandbox = sandbox();
        let gate = Gate::new(&sandbox, "g");
        let (ready, go) = (gate.ready(), sandbox.root().join("g.go"));
        std::thread::scope(|scope| {
            let paused = scope.spawn(|| {
                std::fs::write(&ready, b"").expect("the ready file");
                crate::wait_until("the gate to open", PATIENCE, || go.exists());
            });
            let met = gate.meet(&|| paused.is_finished(), || go.exists());
            assert_eq!(met, Some(false), "the gate opened before the step");
            paused.join().expect("the paused thread");
        });
    }

    #[test]
    fn a_process_that_finishes_without_reaching_the_gate_is_not_met() {
        let sandbox = sandbox();
        let gate = Gate::new(&sandbox, "g");
        assert_eq!(gate.meet(&|| true, || ()), None);
        assert!(
            sandbox.root().join("g.go").exists(),
            "the gate is left open"
        );
    }
}
