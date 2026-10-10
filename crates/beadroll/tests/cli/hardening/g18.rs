//! G18: a definite refusal (a declining hook, a read-only key) is reported at once with the
//! remote's or git's reason, neither retried as a lost race nor reported as no answer
//! (`docs/FINDINGS.md`).

use beadroll_test_support::{Json, Run, parse_json};

/// `_g18_read_only_ssh`: a transport that serves fetches and refuses every push the way a
/// read-only deploy key does.
const READ_ONLY_SSH: &str = concat!(
    "#!/bin/sh\n",
    "if [ \"$1\" = \"-G\" ]; then exit 1; fi\n",
    "for last; do :; done\n",
    "case \"$last\" in\n",
    "  git-receive-pack*) echo 'ERROR: The key you are authenticating with has been marked as read",
    " only.' >&2; exit 1 ;;\n",
    "  git-upload-pack*) eval \"exec $last\" ;;\n",
    "esac\nexit 2\n",
);

/// `_g18_error`: the error a failed `bead ... --json` reported on standard error.
fn error(run: &Run) -> Json {
    let document = parse_json(&run.stderr)
        .unwrap_or_else(|error| panic!("bead's error is not JSON ({error}): {}", run.stderr));
    let error = document.get("error").cloned();
    error.unwrap_or_else(|| panic!("no error in {}", run.stderr))
}

/// The text of `error`'s member `name`, which it must have.
fn text<'e>(error: &'e Json, name: &str) -> &'e str {
    let value = error.get(name).and_then(Json::as_str);
    value.unwrap_or_else(|| panic!("no {name} in {error:?}"))
}

mod g18_definite_refusal_is_neither_a_race_nor_no_answer {
    use std::ffi::OsString;
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use beadroll_test_support::{Json, Machine, Sandbox};

    use super::{READ_ONLY_SSH, error, text};

    /// The class's `setUp`: an empty git config in the sandbox, `g18-empty.gitconfig`, which the
    /// machines read as their global one.
    fn set_up(sandbox: &Sandbox) -> PathBuf {
        let config = sandbox.root().join("g18-empty.gitconfig");
        std::fs::write(&config, "").unwrap();
        config
    }

    /// `machine` reads `config` as its global git config, and no system one.
    fn global(machine: &mut Machine, config: &Path) {
        let config = config.as_os_str().to_owned();
        machine.extra.insert("GIT_CONFIG_GLOBAL".into(), config);
        machine
            .extra
            .insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    }

    /// The key of the issue a `--json` result is.
    fn id(result: Option<Json>) -> String {
        let result = result.expect("bead printed a result");
        let id = result.get("id").and_then(Json::as_str);
        id.unwrap_or_else(|| panic!("no id in {result:?}"))
            .to_owned()
    }

    /// G18: a push the remote refuses by policy (a declining pre-receive hook, '[remote
    /// rejected]') is treated as a lost race: twelve rounds, about twenty seconds, then
    /// SYNC_CONFLICT 'the remote journal kept moving' without the remote's reason.
    #[test]
    fn policy_refusal_is_reported_at_once_with_the_remotes_reason() {
        let sandbox = crate::sandbox();
        let config = set_up(&sandbox);
        let mut a = Machine::new(&sandbox, "g18p");
        global(&mut a, &config);
        a.bead(["init", "--prefix", "po"]);
        // local: the sync below is the push the hook refuses
        let key = id(a.bead(["create", "x", "--offline"]));
        let hook = sandbox.origin().join("hooks").join("pre-receive");
        let script = "#!/bin/sh\necho 'refs/beads/* may not be pushed here' >&2\nexit 1\n";
        std::fs::write(&hook, script).unwrap();
        std::fs::set_permissions(&hook, Permissions::from_mode(0o755)).unwrap();
        let argvs: [&[&str]; 2] = [&["sync"], &["claim", &key]];
        for argv in argvs {
            let started = Instant::now();
            let proc = a.run(argv.iter().copied().chain(["--json"]));
            let elapsed = started.elapsed();
            assert_eq!(proc.returncode(), 1, "{}", proc.stdout);
            let error = error(&proc);
            let code = error.get("code");
            let code = code.unwrap_or_else(|| panic!("no code in {error:?}"));
            assert_ne!(
                code,
                &Json::from("SYNC_CONFLICT"),
                "{}: a refusal by the remote was retried as a race: {error:?} after {:.1}s",
                argv[0],
                elapsed.as_secs_f64()
            );
            assert!(
                text(&error, "message").contains("may not be pushed here"),
                "{} hid the remote's reason",
                argv[0]
            );
            assert!(
                elapsed < Duration::from_secs(5),
                "{} kept retrying a refusal",
                argv[0]
            );
        }
        assert!(a.show(&key).get("claim").is_none());
        std::fs::remove_file(&hook).unwrap();
        assert_eq!(
            a.run(["claim", &key, "--json"]).returncode(),
            0,
            "the refused claim left the issue unclaimable"
        );
    }

    /// G18: a push refused before any ref status (here a read-only ssh key; also HTTP 403, a
    /// pre-push hook) is reported as 'the push got no answer' / 'the remote is not answering', and
    /// git's reason is thrown away.
    #[test]
    fn transport_refusal_reports_gits_reason_not_no_answer() {
        let sandbox = crate::sandbox();
        let config = set_up(&sandbox);
        let mut writer = Machine::new(&sandbox, "g18w");
        global(&mut writer, &config);
        writer.bead(["init", "--prefix", "ex"]);
        let key = id(writer.bead(["create", "one"]));
        writer.bead(["sync"]);
        let ssh = sandbox.root().join("g18-read-only-ssh.sh");
        std::fs::write(&ssh, READ_ONLY_SSH).unwrap();
        std::fs::set_permissions(&ssh, Permissions::from_mode(0o755)).unwrap();
        let mut reader = Machine::new(&sandbox, "g18r");
        global(&mut reader, &config);
        let mut remote = OsString::from("ssh://fakehost");
        remote.push(sandbox.origin());
        reader
            .extra
            .insert("GIT_SSH_COMMAND".into(), ssh.into_os_string());
        reader.extra.insert("BEAD_REMOTE".into(), remote);
        // reading works: only pushing is refused
        reader.bead(["sync"]);
        reader.bead(["comments", "add", &key, "a note from a read-only clone"]);
        let argvs: [&[&str]; 2] = [&["sync"], &["claim", &key]];
        for argv in argvs {
            let started = Instant::now();
            let proc = reader.run(argv.iter().copied().chain(["--json"]));
            assert_eq!(proc.returncode(), 1, "{}", proc.stdout);
            assert!(started.elapsed() < Duration::from_secs(10));
            let error = error(&proc);
            let message = text(&error, "message");
            assert!(
                message.contains("marked as read only"),
                "{} hid git's reason: {message}",
                argv[0]
            );
            assert!(
                !message.contains("no answer") && !message.contains("not answering"),
                "{} says the remote did not answer",
                argv[0]
            );
        }
        assert!(writer.show(&key).get("claim").is_none());
    }
}
