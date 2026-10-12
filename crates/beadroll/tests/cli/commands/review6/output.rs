//! F04, F05, F06 and verified properties: what bead prints and the files it writes for others to
//! read. Files written into a checkout follow the caller's umask while the store stays private, the
//! human output escapes what a terminal would act on, `--json` keeps text exactly, a closed stream
//! leaves the command's own status, and the help pages create no state.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use beadroll_test_support::{Machine, Run, parse_json};

use super::{id, machine, metas};

/// The permission bits of `path`: `stat.S_IMODE(path.stat().st_mode)`.
fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

/// `output` as a [`Run`], decoded as `text=True` decodes it.
fn finished(output: Output) -> Run {
    Run {
        status: output.status,
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// Runs `command` to completion with empty standard input and captures output bytes for lossy
/// text comparison.
fn communicate(mut command: Command) -> Run {
    let child = command.stdin(Stdio::null()).spawn().unwrap();
    // Our copy of the pipe's write end goes with the command, as the test's `os.close(write)`.
    drop(command);
    let output = child.wait_with_output().unwrap();
    Run {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// F04: `.beadroll.json` and `export -o` are not forced owner-only.
#[test]
fn f04_checkout_files_stay_readable_in_a_shared_tree() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "f04");
    let made = machine.run(["init", "--prefix", "ex", "--tracked", "--json"]);
    assert_eq!(made.returncode(), 0, "{}", made.stderr);
    let config = machine.checkout().join(".beadroll.json");
    let target = sandbox.root().join("shared").join("issues.jsonl");
    std::fs::create_dir(target.parent().unwrap()).unwrap();
    let exported = machine.run(["export", "-o", target.to_str().unwrap()]);
    assert_eq!(exported.returncode(), 0, "{}", exported.stderr);
    for path in [&config, &target] {
        let mode = mode(path);
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(mode & 0o040 != 0, "{name} is {mode:#o}");
    }
}

/// F05: human output escapes TAB and Unicode line separators.
#[test]
fn f05_human_output_escapes_tab_and_line_separators() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "f05");
    let created = machine.run(["create", "col\tFORGED\u{2028}line\u{202e}title"]);
    assert_eq!(created.returncode(), 0, "{}", created.stderr);
    for raw in ['\t', '\u{2028}', '\u{202e}'] {
        assert!(
            !created.stdout.contains(raw),
            "{raw:?} in {:?}",
            created.stdout
        );
    }
}

/// F06: a closed stderr on a failed command exits with the command status, not 120.
#[test]
fn f06_failed_command_with_closed_stderr_keeps_its_status() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "f06");
    let (read, write) = std::io::pipe().unwrap();
    drop(read);
    let mut command = machine.cmd(["create"]).command();
    command.stdout(Stdio::piped()).stderr(write);
    let proc = communicate(command);
    assert_eq!(proc.returncode(), 1);
}

/// Verified (no finding): a closed stdout after a successful read exits 0; store files stay
/// private.
#[test]
fn closed_stdout_on_success_exits_zero_and_store_stays_private() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "pipe-ok");
    let key = id(machine.bead(["create", "pipe"]));
    let (read, write) = std::io::pipe().unwrap();
    drop(read);
    let mut command = machine.cmd(["show", &key]).command();
    command.stdout(write).stderr(Stdio::piped());
    let proc = communicate(command);
    assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
    assert!(!proc.crashed(), "{}", proc.stderr);
    let meta = metas(&machine).into_iter().next().expect("a meta.json");
    assert_eq!(
        mode(&meta) & 0o077,
        0,
        "{} is {:#o}",
        meta.display(),
        mode(&meta)
    );
}

/// Verified (no finding): JSON keeps bidi characters in the stored title and escapes them as JSON.
#[test]
fn json_keeps_bidi_text() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "bidi");
    let title = "bidi-\u{202e}JSON";
    let created = machine.run(["create", title, "--json"]);
    assert_eq!(created.returncode(), 0, "{}", created.stderr);
    let result = parse_json(&created.stdout).unwrap();
    assert_eq!(result["title"].as_str(), Some(title));
}

/// Verified (no finding): prime's ready count matches bead ready, and help creates no state.
#[test]
fn prime_matches_ready_and_help_outside_a_repository_is_silent() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "prime");
    let open_id = id(machine.bead(["create", "open"]));
    machine.bead(["create", "blocked", "--deps", &format!("blocks:{open_id}")]);
    let claimed = id(machine.bead(["create", "claimed"]));
    machine.bead(["claim", &claimed, "--offline"]);
    let deferred = id(machine.bead(["create", "deferred"]));
    machine.bead(["update", &deferred, "--status", "deferred"]);
    let closed = id(machine.bead(["create", "closed"]));
    machine.bead(["close", &closed, "--reason", "done"]);
    let epic = id(machine.bead(["create", "epic", "--type", "epic"]));
    machine.bead(["create", "child", "--parent", &epic]);
    let primed = machine.bead(["prime"]).unwrap();
    let primed = primed["prime"].as_str().unwrap();
    let ready = machine.bead(["ready"]).unwrap();
    let ready = ready.as_array().unwrap().len();
    assert!(primed.contains(&format!("{ready} ready")), "{primed}");

    let outside = sandbox.root().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let state = sandbox.root().join("outside-state");
    // The test's own environment without bead's, git's or a harness's variables, and no identity.
    let env: Vec<_> = std::env::vars_os()
        .filter(|(key, _)| {
            let key = key.as_encoded_bytes();
            !["BEAD_", "GIT_", "CLAUDE", "CODEX"]
                .iter()
                .any(|prefix| key.starts_with(prefix.as_bytes()))
        })
        .collect();
    for args in [
        &["help"][..],
        &["help", "claim"],
        &["prime"],
        &["onboard"],
        &["--help"],
    ] {
        let result = Command::new(sandbox.bead())
            .args(args)
            .current_dir(&outside)
            .env_clear()
            .envs(env.iter().cloned())
            .env("BEAD_HOME", &state)
            .output()
            .unwrap();
        let result = finished(result);
        assert_eq!(result.returncode(), 0, "{args:?}: {}", result.stderr);
        assert!(!result.crashed(), "{args:?}: {}", result.stderr);
        assert!(!state.exists(), "{args:?} made {}", state.display());
    }
}
