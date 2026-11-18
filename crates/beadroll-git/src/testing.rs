//! Scratch repositories for this crate's own tests: git in a temporary home, reading no system,
//! global or caller configuration and no caller `GIT_*` variable.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use crate::runner::Env;

/// The options that give git a committer for a commit a test makes, its configuration reading
/// none: `git -c user.name=t -c user.email=t@example.invalid ...`.
pub(crate) const IDENTITY: [&str; 4] = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"];

/// The environment git runs in for a test, starting in `dir`, its home and global configuration in
/// `home`, with `extra` set.
pub(crate) fn clean_env(dir: Option<&Path>, home: &Path, extra: &[(&str, &OsString)]) -> Env {
    let mut vars: BTreeMap<OsString, OsString> = std::env::vars_os()
        .filter(|(name, _)| !name.as_encoded_bytes().starts_with(b"GIT_"))
        .collect();
    vars.insert("HOME".into(), home.as_os_str().to_owned());
    vars.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    vars.insert("GIT_CONFIG_GLOBAL".into(), home.join(".gitconfig").into());
    vars.insert("LC_ALL".into(), "C".into());
    for (name, value) in extra {
        vars.insert((*name).into(), (*value).clone());
    }
    Env::new(vars, dir.map(Path::to_path_buf), std::env::temp_dir())
}

/// A new empty directory, and the environment that runs git there with no repository around it: for a
/// test of the runner itself, which must not find the checkout the test runs in.
pub(crate) fn scratch() -> (tempfile::TempDir, Env) {
    let dir = tempfile::tempdir().unwrap();
    let env = clean_env(Some(dir.path()), dir.path(), &[]);
    (dir, env)
}

/// A bare repository of `format` in a new temporary directory, and the environment that runs git
/// in it (`GIT_DIR`).
pub(crate) fn bare(format: &str) -> (tempfile::TempDir, Env) {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join("r.git");
    let made = clean_env(None, dir.path(), &[]);
    let object_format = format!("--object-format={format}");
    made.git([
        "init",
        "-q",
        "--bare",
        &object_format,
        git_dir.to_str().unwrap(),
    ])
    .check()
    .unwrap();
    let env = clean_env(None, dir.path(), &[("GIT_DIR", &git_dir.into_os_string())]);
    (dir, env)
}
