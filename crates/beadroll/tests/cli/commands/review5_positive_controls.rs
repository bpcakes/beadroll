//! `Review5PositiveControls`: what the fifth independent review (2026-10-07-5-codex) found working,
//! kept working.

use std::process::Command;

use beadroll_test_support::Run;

/// V02: top-level and subcommand help are local, side-effect free operations.
#[test]
fn v02_help_needs_no_repository_and_creates_no_state() {
    // The sandbox names the bead under test; the help runs in a new directory of its own, in no
    // repository, with the test's whole environment and a home that does not exist.
    let sandbox = crate::sandbox();
    let root = sandbox.root().join("bead-review5-help");
    std::fs::create_dir(&root).unwrap();
    let state = root.join("state");
    for args in [&["--help"][..], &["create", "--help"]] {
        let output = Command::new(sandbox.bead())
            .args(args)
            .current_dir(&root)
            .env("BEAD_HOME", &state)
            .output()
            .expect("bead runs");
        let result = Run {
            status: output.status,
            stdout: String::from_utf8(output.stdout).expect("UTF-8 output"),
            stderr: String::from_utf8(output.stderr).expect("UTF-8 errors"),
        };
        assert_eq!(result.returncode(), 0, "{args:?}: {}", result.stderr);
        assert!(
            result.stdout.to_lowercase().contains("usage:"),
            "{args:?}: {}",
            result.stdout
        );
        assert!(!state.exists(), "{args:?} created {}", state.display());
    }
}
