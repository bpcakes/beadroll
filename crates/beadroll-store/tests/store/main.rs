//! The Python suite's tests of what beadroll-store owns (`docs/RUST-PORT.md` §6, Phase 1's list of
//! B-class tests that land with Phase 2a): the coordination ref's rules, two workspaces on one
//! machine, actor detection, the remote a workspace remembers, and attempt files read back. Named by
//! the inventory's rule (`xtask/inventory.txt`).
#![cfg(test)]

mod commands;
mod compat;
mod hardening;
mod replay;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use beadroll_store::Settings;

/// Settings as a `bead` run with `vars` (and the test's own environment, less anything of the
/// developer's harness, store or repository) would have them, its store at `home`.
pub(crate) fn settings(home: &Path, vars: &[(&str, &str)]) -> Arc<Settings> {
    let mut all: BTreeMap<OsString, OsString> = std::env::vars_os()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            !["BEAD_", "CLAUDE", "CODEX", "GIT_"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .collect();
    all.insert("BEAD_HOME".into(), home.as_os_str().to_owned());
    for (name, value) in vars {
        all.insert((*name).into(), (*value).into());
    }
    let user_home = std::env::var_os("HOME").unwrap_or_default();
    let cwd = std::env::current_dir().ok();
    Arc::new(Settings::from_process(all, cwd, user_home, std::env::temp_dir(), &|_| None).unwrap())
}
