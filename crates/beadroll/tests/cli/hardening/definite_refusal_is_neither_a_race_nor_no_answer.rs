//! G18: a definite refusal (a declining hook, a read-only key) is reported at once with the
//! remote's or git's reason, neither retried as a lost race nor reported as no answer
//! (`docs/FINDINGS.md`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use beadroll_test_support::{Json, Machine, Run, Sandbox, executable, parse_json, printed_id};

/// A transport that serves fetches and refuses every push the way a
/// read-only deploy key does, noting each push it refuses in the file `G18_PUSHES` names.
const READ_ONLY_SSH: &str = concat!(
    "#!/bin/sh\n",
    "if [ \"$1\" = \"-G\" ]; then exit 1; fi\n",
    "for last; do :; done\n",
    "case \"$last\" in\n",
    "  git-receive-pack*) echo refused >> \"$G18_PUSHES\"; echo 'ERROR: The key you are authenticating with has been marked as read",
    " only.' >&2; exit 1 ;;\n",
    "  git-upload-pack*) eval \"exec $last\" ;;\n",
    "esac\nexit 2\n",
);

/// How many lines the file at `path` holds: the pushes a counting hook or transport noted.
fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .count()
}

/// The error a failed `bead ... --json` reported on standard error.
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

/// An empty git config in the sandbox, `g18-empty.gitconfig`, which the
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
    let key = printed_id(a.bead(["create", "x", "--offline"]));
    let hook = sandbox.origin().join("hooks").join("pre-receive");
    // the hook counts the pushes it refuses: one a command, where a race would make many
    let refusals = sandbox.root().join("g18-refusals");
    let script = format!(
        "#!/bin/sh\necho refused >> '{}'\necho 'refs/beads/* may not be pushed here' >&2\nexit 1\n",
        refusals.display()
    );
    executable(&hook, script);
    let argvs: [&[&str]; 2] = [&["sync"], &["claim", &key]];
    for argv in argvs {
        let before = lines(&refusals);
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
        assert_eq!(
            lines(&refusals) - before,
            1,
            "{} kept retrying a refusal ({:.1}s)",
            argv[0],
            elapsed.as_secs_f64()
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
    let key = printed_id(writer.bead(["create", "one"]));
    writer.bead(["sync"]);
    let ssh = sandbox.root().join("g18-read-only-ssh.sh");
    executable(&ssh, READ_ONLY_SSH);
    let mut reader = Machine::new(&sandbox, "g18r");
    global(&mut reader, &config);
    let mut remote = OsString::from("ssh://fakehost");
    remote.push(sandbox.origin());
    reader
        .extra
        .insert("GIT_SSH_COMMAND".into(), ssh.into_os_string());
    reader.extra.insert("BEAD_REMOTE".into(), remote);
    let pushes = sandbox.root().join("g18-pushes");
    reader
        .extra
        .insert("G18_PUSHES".into(), pushes.clone().into_os_string());
    // reading works: only pushing is refused
    reader.bead(["sync"]);
    reader.bead(["comments", "add", &key, "a note from a read-only clone"]);
    // Not retried as a race (24 rounds): a sync pushes once; a claim pushes its attempt, which a
    // refusal without any ref status leaves pending, and then the one fence that would settle it.
    let argvs: [(&[&str], usize); 2] = [(&["sync"], 1), (&["claim", &key], 2)];
    for (argv, expected) in argvs {
        let before = lines(&pushes);
        let proc = reader.run(argv.iter().copied().chain(["--json"]));
        assert_eq!(proc.returncode(), 1, "{}", proc.stdout);
        assert_eq!(
            lines(&pushes) - before,
            expected,
            "{} retried a refused push: {}",
            argv[0],
            proc.stderr
        );
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
