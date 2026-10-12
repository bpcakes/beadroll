//! G19: the user's client hooks never run on bead's private pushes; the remote's hooks still apply
//! (`docs/FINDINGS.md`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// `_g19_git`: git ARGS as user `t`, in `cwd`, with exactly the environment `env`; its output,
/// trimmed. A failure fails the test with git's standard error.
fn git(args: &[&str], cwd: &Path, env: &BTreeMap<OsString, OsString>) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .output()
        .expect("git starts");
    assert!(
        output.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("git's output is UTF-8");
    stdout.trim().to_owned()
}

/// G19: bead's private journal pushes run the user's client hooks: `git init` copies
/// init.templateDir's hooks into $BEAD_HOME/<ws>/sync.git (where they stay), and a global
/// core.hooksPath or a config-based hook applies to the push. A hook that fails there makes every
/// push 'uncertain', so every claim ends UNRESOLVED and every sync REMOTE_UNREACHABLE ('the push got
/// no answer'), the hook's message is never shown, and nothing changes until the hook is found in a
/// directory the user does not know about.
mod g19_client_hooks_on_private_pushes {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use beadroll_store::glob_ending;
    use beadroll_test_support::{Json, Machine, Sandbox};

    use super::git;

    /// The class's fixture: a sandbox, the log every hook writes to (`hook.log`), a scratch global
    /// git config (`global.gitconfig`, never the developer's), and machine a, which reads it as
    /// its global config and reads no system one.
    struct Hooks {
        sandbox: Sandbox,
        log: PathBuf,
        config: PathBuf,
        a: Machine,
    }

    impl Hooks {
        /// The class's `setUp`.
        fn set_up() -> Hooks {
            let sandbox = crate::sandbox();
            let log = sandbox.root().join("hook.log");
            let config = sandbox.root().join("global.gitconfig");
            std::fs::write(&config, "").unwrap();
            let mut a = Machine::new(&sandbox, "a");
            let global = config.as_os_str().to_owned();
            a.extra.insert("GIT_CONFIG_GLOBAL".into(), global);
            a.extra.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
            Hooks {
                sandbox,
                log,
                config,
                a,
            }
        }

        /// `hook`: an executable script at `path` that notes in the log that it ran, and where,
        /// then runs `body`; its path.
        fn hook(&self, path: &Path, body: &str) -> PathBuf {
            std::fs::create_dir_all(path.parent().expect("the hook's folder")).unwrap();
            let log = self.log.to_str().expect("a UTF-8 path");
            let script = format!("#!/bin/sh\necho \"$0 ran in $PWD\" >> '{log}'\n{body}\n");
            std::fs::write(path, script).unwrap();
            std::fs::set_permissions(path, Permissions::from_mode(0o755)).unwrap();
            path.to_path_buf()
        }

        /// `ran`: what the hooks noted, nothing when none has run.
        fn ran(&self) -> String {
            if self.log.exists() {
                std::fs::read_to_string(&self.log).unwrap()
            } else {
                String::new()
            }
        }

        /// The global git config now says `text`.
        fn configure(&self, text: &str) {
            std::fs::write(&self.config, text).unwrap();
        }

        /// `assert_claim_and_sync_work`: claiming `issue` and syncing both succeed, settle every
        /// attempt, and run no hook.
        fn assert_claim_and_sync_work(&self, issue: &str) {
            let claim = self.a.run(["claim", issue, "--json"]);
            assert_eq!(
                claim.returncode(),
                0,
                "claim: {} (hook log: {:?})",
                claim.stderr,
                self.ran()
            );
            let sync = self.a.run(["sync", "--json"]);
            assert_eq!(
                sync.returncode(),
                0,
                "sync: {} (hook log: {:?})",
                sync.stderr,
                self.ran()
            );
            assert_eq!(
                glob_ending(&self.a.store().join("pending"), ".json"),
                Vec::<PathBuf>::new(),
                "an attempt stayed unsettled"
            );
            assert_eq!(self.ran(), "", "a client hook ran on bead's private push");
        }

        /// Machine a initialises tracker `hk` and creates an issue: its key.
        fn tracker(&self) -> String {
            self.a.bead(["init", "--prefix", "hk"]);
            let created = self.a.bead(["create", "one"]).expect("the new issue");
            let key = created.get("id").and_then(Json::as_str);
            key.expect("the issue's id").to_owned()
        }
    }

    /// G19: a pre-push hook from init.templateDir lands in sync.git at first contact and keeps
    /// blocking every push after the user has dropped the setting.
    #[test]
    fn template_hook_is_not_copied_into_sync_git() {
        let h = Hooks::set_up();
        let template = h.sandbox.root().join("template");
        let blocked = "echo 'scanner: push blocked' >&2\nexit 1";
        h.hook(&template.join("hooks").join("pre-push"), blocked);
        let template = template.to_str().expect("a UTF-8 path");
        h.configure(&format!("[init]\n\ttemplateDir = {template}\n"));
        let issue = h.tracker();
        // the first contact creates sync.git under the user's config
        h.a.run(["sync", "--json"]);
        // the user drops the template; a copy in sync.git would stay behind
        h.configure("");
        let copied = h.a.store().join("sync.git").join("hooks").join("pre-push");
        assert!(
            !copied.exists(),
            "the user's template hook was copied to {}",
            copied.display()
        );
        h.assert_claim_and_sync_work(&issue);
    }

    /// G19: a global core.hooksPath pre-push hook that passes for the user's own pushes fails on
    /// bead's (it runs against the bare sync.git), and every claim then ends UNRESOLVED.
    #[test]
    fn global_hook_written_for_code_pushes_does_not_block_claims() {
        let h = Hooks::set_up();
        let hooks = h.sandbox.root().join("hooks");
        let body = "git rev-parse --show-toplevel >/dev/null || exit 1";
        h.hook(&hooks.join("pre-push"), body);
        let issue = h.tracker();
        h.a.bead(["sync"]);
        let hooks = hooks.to_str().expect("a UTF-8 path");
        h.configure(&format!("[core]\n\thooksPath = {hooks}\n"));
        let env = h.a.env(None, "codex");
        let commit = ["commit", "-q", "--allow-empty", "-m", "work"];
        git(&commit, h.a.checkout(), &env);
        // passes the hook
        let push = ["push", "-q", "origin", "HEAD:refs/heads/topic"];
        git(&push, h.a.checkout(), &env);
        assert!(
            h.ran().contains("ran in"),
            "the hook did not run on the user's own push"
        );
        std::fs::remove_file(&h.log).unwrap();
        h.assert_claim_and_sync_work(&issue);
    }

    /// G19: a pre-push hook an earlier bead copied into the store blocks every later push.
    #[test]
    fn hook_already_in_an_existing_sync_git_is_not_run() {
        let h = Hooks::set_up();
        let issue = h.tracker();
        h.a.bead(["sync"]);
        let hook = h.a.store().join("sync.git").join("hooks").join("pre-push");
        h.hook(&hook, "echo 'policy: refused' >&2\nexit 1");
        h.assert_claim_and_sync_work(&issue);
    }

    /// G19: config-based pre-push hooks in git >= 2.54 are not governed by `core.hooksPath`. Skip
    /// if `git hook list` does not support such a hook.
    #[test]
    fn config_based_global_hook_does_not_block_claims() {
        let h = Hooks::set_up();
        let refusing = "echo 'policy: refused' >&2\nexit 1";
        let script = h.hook(&h.sandbox.root().join("policy-hook"), refusing);
        let script = script.to_str().expect("a UTF-8 path");
        let text = format!("[hook \"policy\"]\n\tevent = pre-push\n\tcommand = {script}\n");
        let probe_config = h.sandbox.root().join("probe.gitconfig");
        std::fs::write(&probe_config, &text).unwrap();
        let inherited =
            std::env::vars_os().filter(|(k, _)| !k.as_encoded_bytes().starts_with(b"GIT_"));
        let listed = Command::new("git")
            .arg("--git-dir")
            .arg(h.sandbox.origin())
            .args(["hook", "list", "pre-push"])
            .current_dir(h.sandbox.root())
            .env_clear()
            .envs(inherited)
            .env("GIT_CONFIG_GLOBAL", &probe_config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git starts");
        if !String::from_utf8_lossy(&listed.stdout).contains("policy") {
            eprintln!("skipped: this git has no config-based hooks");
            return;
        }
        let issue = h.tracker();
        h.a.bead(["sync"]);
        h.configure(&text);
        h.assert_claim_and_sync_work(&issue);
    }
}
