//! G05 (`docs/FINDINGS.md`): `host:path`, ssh aliases, `gh:` shortcuts, `[::1]:path`, upper-case
//! schemes and `transport::` origins were joined onto the checkout as paths: one tracker per clone.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use beadroll_test_support::{Json, Machine, Sandbox, parse_json};

/// `_g05_fake_ssh`: an ssh stand-in for git (`GIT_SSH_COMMAND`): it runs the remote command in
/// `$FAKE_SSH_ROOT`, locally.
fn fake_ssh(sandbox: &Sandbox) -> PathBuf {
    let path = sandbox.root().join("fake-ssh");
    let script = concat!(
        "#!/bin/sh\n",
        "[ \"$1\" = \"-G\" ] && exit 0\n",
        "for last; do :; done\n",
        "PATH=\"$(git --exec-path):$PATH\"\n",
        "cd \"$FAKE_SSH_ROOT\" && exec sh -c \"$last\"\n",
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, Permissions::from_mode(0o755)).unwrap();
    path
}

/// `_g05_git_env`: a scratch global git config for this sandbox (never the developer's own),
/// holding `text`, and the variables that make git read it alone.
fn git_env(sandbox: &Sandbox, text: &str) -> BTreeMap<OsString, OsString> {
    let entries = std::fs::read_dir(sandbox.root()).unwrap();
    let taken = entries
        .filter(|entry| {
            let name = entry.as_ref().unwrap().file_name();
            name.as_encoded_bytes().starts_with(b"gitconfig-")
        })
        .count();
    let path = sandbox.root().join(format!("gitconfig-{taken}"));
    std::fs::write(&path, text).unwrap();
    BTreeMap::from([
        ("GIT_CONFIG_GLOBAL".into(), path.into_os_string()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ])
}

/// `_g05_info`: what `bead info --json` reports on `machine`, run in `cwd` (its checkout when
/// `None`); a failure fails the test.
fn info(machine: &Machine, cwd: Option<&Path>) -> Json {
    let mut invocation = machine.cmd(["info", "--json"]);
    if let Some(cwd) = cwd {
        invocation = invocation.cwd(cwd);
    }
    let run = invocation.run();
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    parse_json(&run.stdout).unwrap_or_else(|error| panic!("{error}: {}", run.stdout))
}

mod g05_origins_git_reads_as_remotes {
    use std::process::Command;

    use beadroll_test_support::{Json, Machine, git};

    use super::{fake_ssh, git_env, info};

    /// The member `key` of `value`; absence fails the test.
    fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
        value
            .get(key)
            .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
    }

    /// G05: an origin git reads as ssh or a remote helper (host:path without user@, an ssh alias,
    /// an insteadOf shorthand such as gh:org/repo, bracketed IPv6 scp, an upper-case scheme,
    /// transport::address) is not a path under the checkout: every clone, worktree and
    /// subdirectory gets one workspace and the remote is the URL itself.
    #[test]
    fn origin_shapes_git_reads_as_remotes_are_one_workspace() {
        let sandbox = crate::sandbox();
        let one = Machine::new(&sandbox, "one");
        let two = Machine::new(&sandbox, "two");
        let worktree = sandbox.root().join("one-wt");
        let worktree_text = worktree.to_str().expect("a UTF-8 path");
        git(
            ["worktree", "add", "-q", worktree_text],
            Some(one.checkout()),
        );
        let subdirectory = one.checkout().join("sub");
        std::fs::create_dir(&subdirectory).unwrap();
        let urls = [
            "myhost:org/repo.git",
            "host.example:org/repo.git",
            "gh:org/repo",
            "[::1]:org/repo.git",
            "HTTPS://host.example/org/repo.git",
            "codecommit::us-east-1://my-repo",
        ];
        for url in urls {
            for machine in [&one, &two] {
                git(
                    ["remote", "set-url", "origin", url],
                    Some(machine.checkout()),
                );
            }
            let seen = [
                ("clone", info(&one, None)),
                ("worktree", info(&one, Some(&worktree))),
                ("subdirectory", info(&one, Some(&subdirectory))),
                ("other clone", info(&two, None)),
            ];
            let ids: Vec<(&str, &Json)> = seen
                .iter()
                .map(|(place, info)| (*place, member(info, "workspace")))
                .collect();
            assert!(
                ids.iter().all(|(_, id)| *id == ids[0].1),
                "origin={url}: git reads {url:?} as a remote, yet each checkout got its own \
                 tracker: {ids:?}"
            );
            for (place, info) in &seen {
                assert_eq!(
                    member(info, "remote"),
                    &Json::from(url),
                    "origin={url}: {place}: the remote became a path under the checkout"
                );
            }
        }
    }

    /// G05: HTTPS://Host/org/repo and https://host/org/repo are one remote, so one workspace.
    #[test]
    fn upper_case_scheme_is_the_same_remote() {
        let sandbox = crate::sandbox();
        let machine = Machine::new(&sandbox, "case");
        let mut ids = Vec::new();
        for url in [
            "https://host.example/org/repo.git",
            "HTTPS://host.example/org/repo.git",
        ] {
            git(
                ["remote", "set-url", "origin", url],
                Some(machine.checkout()),
            );
            ids.push(member(&info(&machine, None), "workspace").clone());
        }
        assert_eq!(
            ids[0], ids[1],
            "an upper-case scheme gave the same remote a different workspace"
        );
    }

    /// G05: `myhost:origin.git` (an ssh alias, no user@) and `gh:origin.git` (an insteadOf
    /// shorthand) are remotes git reaches; bead must sync through them and a second clone must see
    /// the issue.
    #[test]
    fn scp_and_shorthand_origins_sync() {
        let sandbox = crate::sandbox();
        let ssh = fake_ssh(&sandbox);
        let mut alias = git_env(&sandbox, "");
        alias.insert("GIT_SSH_COMMAND".into(), ssh.into_os_string());
        alias.insert(
            "FAKE_SSH_ROOT".into(),
            sandbox.root().as_os_str().to_owned(),
        );
        let rewrite = format!(
            "[url \"{}/\"]\n\tinsteadOf = gh:\n",
            sandbox.root().display()
        );
        let shapes = [
            ("ssh-alias", "myhost:{name}.git", alias),
            ("insteadof", "gh:{name}.git", git_env(&sandbox, &rewrite)),
        ];
        for (shape, template, env) in shapes {
            let name = format!("tracker-{shape}");
            let url = template.replace("{name}", &name);
            sandbox.bare(&format!("{name}.git"));
            let mut a = Machine::new(&sandbox, &format!("{shape}-a"));
            let mut b = Machine::new(&sandbox, &format!("{shape}-b"));
            for machine in [&mut a, &mut b] {
                git(
                    ["remote", "set-url", "origin", &url],
                    Some(machine.checkout()),
                );
                machine.extra.extend(env.clone());
            }
            let inherited =
                std::env::vars_os().filter(|(key, _)| !key.as_encoded_bytes().starts_with(b"GIT_"));
            let reached = Command::new("git")
                .args(["ls-remote", "origin"])
                .current_dir(a.checkout())
                .env_clear()
                .envs(inherited)
                .envs(&env)
                .output()
                .expect("git starts");
            assert_eq!(
                reached.status.code(),
                Some(0),
                "shape={shape}: git itself cannot reach {url}: {}",
                String::from_utf8_lossy(&reached.stderr)
            );
            let made = a.run(["init", "--prefix", "ex"]);
            assert_eq!(made.returncode(), 0, "shape={shape}: {}", made.stderr);
            let created = a.bead(["create", "shared through the remote"]);
            let created = created.expect("bead printed the issue it made");
            let key = created["id"].as_str().expect("an id");
            let synced = a.run(["sync", "--json"]);
            assert_eq!(
                synced.returncode(),
                0,
                "shape={shape}: bead could not sync through {url}: {}",
                synced.stderr
            );
            let pulled = b.run(["sync", "--json"]);
            assert_eq!(pulled.returncode(), 0, "shape={shape}: {}", pulled.stderr);
            assert_eq!(
                member(&b.show(key), "title"),
                &Json::from("shared through the remote"),
                "shape={shape}"
            );
        }
    }
}
