//! Exercise real fixture callers in a child test process whose Git environment points elsewhere.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{UNDER_TEST, git, machine::scrubbed};

/// Run named tests from the current test binary under inherited repository and object routing.
/// Each must pass without changing any file in an unrelated repository with staged work. Only
/// the child receives the hostile environment: tests in this process remain isolated. This
/// helper needs no bead binary; tests that run bead choose their own through their fixtures.
pub fn assert_git_isolation(tests: &[&str]) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let checkout = root.join("victim");
    std::fs::create_dir(&checkout).unwrap();
    git(["init", "-q", "-b", "main"], Some(&checkout));
    git(
        [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "victim",
        ],
        Some(&checkout),
    );
    std::fs::write(checkout.join("staged-work"), "not a fixture commit\n").unwrap();
    git(["add", "staged-work"], Some(&checkout));
    let git_dir = checkout.join(".git");
    let before = snapshot(&root);
    let test_binary = std::env::current_exe().unwrap();
    let mut environment = scrubbed(std::env::vars_os());
    // This process runs tests, whose fixtures still need the caller's binary selection.
    // Machine removes the selector when it starts bead itself.
    if let Some(selected) = std::env::var_os(UNDER_TEST) {
        environment.insert(UNDER_TEST.into(), selected);
    }
    for (routing, variables) in [
        (
            "repository",
            vec![
                ("GIT_DIR", git_dir.clone()),
                ("GIT_WORK_TREE", checkout),
                ("GIT_COMMON_DIR", git_dir.clone()),
                ("GIT_INDEX_FILE", git_dir.join("index")),
            ],
        ),
        (
            "objects",
            vec![("GIT_OBJECT_DIRECTORY", git_dir.join("objects"))],
        ),
    ] {
        for test in tests {
            let output = Command::new(&test_binary)
                .args(["--exact", test, "--nocapture"])
                .env_clear()
                .envs(&environment)
                .env("BEAD_HOME", root.join("home"))
                .envs(variables.iter().cloned())
                .output()
                .unwrap();
            let after = snapshot(&root);
            let changed: Vec<_> = before
                .keys()
                .chain(after.keys())
                .filter(|path| before.get(*path) != after.get(*path))
                .collect();
            assert!(
                changed.is_empty(),
                "{test} with inherited {routing} routing changed unrelated files: {changed:?}"
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success() && stdout.contains("test result: ok. 1 passed;"),
                "{test} with inherited {routing} routing: {stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

/// Every file's bytes, including the index, configuration, refs and objects. Symlinks are recorded
/// without following them, so even a template's link cannot take the probe outside its sandbox.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, at: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(at).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                visit(root, &path, files);
            } else {
                let bytes = if kind.is_symlink() {
                    std::fs::read_link(&path)
                        .unwrap()
                        .as_os_str()
                        .as_encoded_bytes()
                        .to_vec()
                } else {
                    std::fs::read(&path).unwrap()
                };
                files.insert(path.strip_prefix(root).unwrap().to_path_buf(), bytes);
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}
