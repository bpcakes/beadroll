//! The handshake of the `pause-until(PATH)` failpoint (`docs/RUST-PORT.md` §4.5) with a `bead`
//! that runs beside the test: bead touches `PATH.ready` when it reaches the point, then waits for
//! `PATH.go`, which the test touches once it has done what that moment is for. It stands in for
//! the Python tests' patched functions that ran another command in the middle of one in process.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use beadroll_test_support::{Run, Sandbox};

/// How long a test waits for bead to reach a gate before it gives up on it.
const PATIENCE: Duration = Duration::from_secs(120);

/// A gate a `pause-until` failpoint stops at, named in the sandbox.
pub(crate) struct Gate {
    path: PathBuf,
}

impl Gate {
    /// The gate `name` in `sandbox`.
    pub(crate) fn new(sandbox: &Sandbox, name: &str) -> Gate {
        Gate {
            path: sandbox.root().join(name),
        }
    }

    /// The failpoint action that stops at this gate: `pause-until(PATH)`.
    pub(crate) fn action(&self) -> String {
        format!("pause-until({})", self.path.display())
    }

    /// Once bead has reached the gate, runs `step` and then opens the gate, even if `step` fails:
    /// what `step` came to, or `None` when bead finished (`finished`) without reaching it. The gate
    /// is opened however this ends, so that bead never waits out its limit for a test that gave
    /// up; once open it stays open, and bead passes any later hit of the point at once.
    pub(crate) fn meet<T>(
        &self,
        finished: &dyn Fn() -> bool,
        step: impl FnOnce() -> T,
    ) -> Option<T> {
        let ready = self.with(".ready");
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

    /// The gate's path with `suffix` added to its name.
    fn with(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    }
}

/// Touches its file when dropped: the gate opened, whatever the test came to.
struct Open(PathBuf);

impl Drop for Open {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, b"");
    }
}

/// Runs `bead` (a run of it, to the end) in a thread of its own while `test` meets it at its gates,
/// told whether it has finished: what each came to.
pub(crate) fn beside<T>(
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
