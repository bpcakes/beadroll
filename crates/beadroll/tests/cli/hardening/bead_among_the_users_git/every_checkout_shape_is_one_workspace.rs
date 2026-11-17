//! Every shape a checkout of one repository can take resolves to its one URL-derived workspace.

use beadroll_test_support::Machine;

use super::{git_env, scratch_git_config, sh};

/// Verified (V9): with an untracked, URL-derived workspace, every checkout shape of one repository
/// resolves to the same workspace and reads the same issue: a subdirectory, a linked worktree, a
/// detached worktree, a bare clone as cwd, a shallow clone (depth 1), an unborn HEAD with the same
/// origin, and a submodule checkout of the repository. The review measured 7/7 places agreeing
/// (with a tracked .beadroll.json, 7 of 11 places agree; the other 4 are its finding G06 and are
/// not tested here).
#[test]
fn checkout_shapes_of_one_repository_share_the_workspace() {
    let sandbox = crate::sandbox();
    let root = sandbox.root();
    let genv = scratch_git_config(&sandbox, "");
    let env = git_env(&genv);
    let mut a = Machine::new(&sandbox, "a");
    a.extra.extend(genv);
    a.bead(["init"]);
    let created = a.bead(["create", "one"]).expect("the created issue");
    let issue = created["id"].as_str().expect("an id").to_owned();
    a.bead(["sync"]);
    let info = a.bead(["info"]).expect("info");
    let workspace = info["workspace"].as_str().expect("a workspace").to_owned();
    std::fs::create_dir(a.checkout().join("src")).unwrap();
    let path = |name: &str| root.join(name).display().to_string();
    let origin = sandbox.origin().display().to_string();
    let checkout = Some(a.checkout());
    sh(
        &["worktree", "add", "-q", &path("wt"), "-b", "feature"],
        checkout,
        &env,
    );
    sh(
        &["worktree", "add", "-q", "--detach", &path("detached")],
        checkout,
        &env,
    );
    sh(
        &["clone", "-q", "--bare", &origin, &path("bare.git")],
        None,
        &env,
    );
    let file_url = format!("file://{origin}");
    sh(
        &["clone", "-q", "--depth", "1", &file_url, &path("shallow")],
        None,
        &env,
    );
    let shallow = root.join("shallow");
    sh(
        &["remote", "set-url", "origin", &origin],
        Some(&shallow),
        &env,
    );
    sh(&["init", "-q", &path("unborn")], None, &env);
    let unborn = root.join("unborn");
    sh(&["remote", "add", "origin", &origin], Some(&unborn), &env);
    sh(&["init", "-q", &path("super")], None, &env);
    let superproject = root.join("super");
    sh(
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &origin,
            "lib",
        ],
        Some(&superproject),
        &env,
    );
    let places = [
        a.checkout().join("src"),
        root.join("wt"),
        root.join("detached"),
        root.join("bare.git"),
        shallow,
        unborn,
        superproject.join("lib"),
    ];
    for place in places {
        let here = a.cmd(["info"]).cwd(&place).json().expect("info");
        assert_eq!(
            here["workspace"].as_str(),
            Some(workspace.as_str()),
            "{}",
            place.display()
        );
        let shown = a.cmd(["show", &issue]).cwd(&place).json().expect("show");
        assert_eq!(
            shown[0]["title"].as_str(),
            Some("one"),
            "{}",
            place.display()
        );
    }
}
