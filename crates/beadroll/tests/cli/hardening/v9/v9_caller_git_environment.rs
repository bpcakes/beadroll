//! The git environment a hook, an alias or `git -c` hands bead changes neither its workspace nor
//! its claims.

use beadroll_test_support::{Machine, parse_json};

use super::scratch_git_config;

/// Verified (V9): a caller's git environment as hooks, aliases and `git -c` set it does not
/// change the workspace or break claims once sync.git exists. Cases: GIT_DIR alone (as a hook in a
/// linked worktree gets it, cwd at the worktree top), GIT_DIR+GIT_WORK_TREE,
/// GIT_INDEX_FILE+GIT_PREFIX (pre-commit), GIT_CONFIG_PARAMETERS, fixed
/// GIT_COMMITTER_DATE/GIT_AUTHOR_DATE, and GIT_TRACE=1. The review measured 6/6 cases with the
/// same workspace and claim and release rc 0 (git 2.55).
#[test]
fn hook_and_alias_git_environment_keeps_workspace_and_claims() {
    let sandbox = crate::sandbox();
    let mut a = Machine::new(&sandbox, "a");
    a.extra.extend(scratch_git_config(&sandbox, ""));
    a.bead(["init"]);
    let created = a.bead(["create", "one"]).expect("the created issue");
    let issue = created["id"].as_str().expect("an id").to_owned();
    a.bead(["sync"]);
    let info = a.bead(["info"]).expect("info");
    let workspace = info["workspace"].as_str().expect("a workspace").to_owned();
    std::fs::create_dir(a.checkout().join("src")).unwrap();
    let git_dir = a.checkout().join(".git").display().to_string();
    let work_tree = a.checkout().display().to_string();
    let cases: [&[(&str, &str)]; 6] = [
        &[("GIT_DIR", &git_dir)],
        &[("GIT_DIR", &git_dir), ("GIT_WORK_TREE", &work_tree)],
        &[("GIT_INDEX_FILE", ".git/index"), ("GIT_PREFIX", "src/")],
        &[("GIT_CONFIG_PARAMETERS", "'user.name'='t'")],
        &[
            ("GIT_COMMITTER_DATE", "2001-01-01T00:00:00Z"),
            ("GIT_AUTHOR_DATE", "2001-01-01T00:00:00Z"),
        ],
        &[("GIT_TRACE", "1")],
    ];
    for extra in cases {
        let run = |args: &[&str]| {
            let mut invocation = a.cmd(args);
            for (name, value) in extra {
                invocation = invocation.env(*name, *value);
            }
            invocation.run()
        };
        let info = run(&["info", "--json"]);
        assert_eq!(info.returncode(), 0, "{extra:?}: {}", info.stderr);
        let shown = parse_json(&info.stdout).expect("info prints JSON");
        assert_eq!(
            shown["workspace"].as_str(),
            Some(workspace.as_str()),
            "{extra:?}"
        );
        let claim = run(&["claim", &issue, "--json"]);
        assert_eq!(claim.returncode(), 0, "{extra:?}: {}", claim.stderr);
        let release = run(&["release", &issue, "--json"]);
        assert_eq!(release.returncode(), 0, "{extra:?}: {}", release.stderr);
    }
}
