//! Settings name the checkout git runs in: the Rust crates' own contract (`docs/RUST-PORT.md`
//! §2.2), not a ported Python test.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::Arc;

use beadroll_store::{Settings, find_workspace};

/// A library caller that names a checkout other than the process's working directory finds
/// that checkout's tracker: git runs where the settings say, not where the process happens to
/// be (not a Python test: the Rust crates' own contract, `docs/RUST-PORT.md` §2.2).
#[test]
fn discovery_runs_git_in_the_checkout_the_settings_name() {
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().canonicalize().unwrap().join("elsewhere");
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&checkout)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    std::fs::create_dir(&checkout).unwrap();
    git(&["init", "-q"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://example.invalid/org/elsewhere.git",
    ]);
    let vars: BTreeMap<OsString, OsString> = [
        (
            OsString::from("BEAD_HOME"),
            root.path().join("home").into_os_string(),
        ),
        (
            OsString::from("PATH"),
            std::env::var_os("PATH").unwrap_or_default(),
        ),
    ]
    .into_iter()
    .collect();
    let settings = Settings::from_process(
        vars,
        Some(checkout.clone()),
        OsString::new(),
        std::env::temp_dir(),
        &|_| None,
    )
    .unwrap();
    let ws = find_workspace(&Arc::new(settings)).unwrap();
    assert_eq!(
        ws.origin,
        Some(OsString::from("https://example.invalid/org/elsewhere.git"))
    );
    assert_eq!(ws.name, Some(OsString::from("elsewhere")));
}
