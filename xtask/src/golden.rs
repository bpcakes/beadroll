//! `cargo xtask golden`: the golden outputs (`crates/beadroll/tests/golden/`) captured from Python's
//! `./bead`, and only from it (`docs/RUST-PORT.md` §4.2 item 5). The scenarios and their
//! normalisation live in the golden tests themselves (`crates/beadroll/tests/cli/golden.rs`); this
//! runs them in capture mode against the launcher, so capturing and checking cannot drift apart.

use std::ffi::OsString;
use std::process::{Command, ExitCode};

use crate::workspace::{self, Failure};

pub(crate) fn main(args: Vec<OsString>) -> Result<ExitCode, Failure> {
    if let Some(arg) = args.first() {
        return Err(Failure::Usage(format!(
            "golden takes no arguments, not {}",
            arg.to_string_lossy()
        )));
    }
    let root = workspace::root();
    let launcher = root.join("bead");
    let status = Command::new(workspace::cargo())
        .args([
            "test",
            "--locked",
            "--package",
            "beadroll",
            "--test",
            "cli",
            "--",
            "golden::",
            "--test-threads",
            "1",
        ])
        .env("BEAD_UNDER_TEST", &launcher)
        .env("BEAD_GOLDEN_CAPTURE", "1")
        // the oracle's refusal would stop the launcher: this run is Python's on purpose
        .env_remove("BEADROLL_ORACLE_REFUSALS")
        .current_dir(&root)
        .status()
        .map_err(|error| Failure::Failed(format!("cannot run cargo: {error}")))?;
    if !status.success() {
        return Err(Failure::Failed(format!(
            "capturing the goldens from {} failed ({status})",
            launcher.display()
        )));
    }
    println!(
        "goldens captured from {} into crates/beadroll/tests/golden/",
        launcher.display()
    );
    Ok(ExitCode::SUCCESS)
}
