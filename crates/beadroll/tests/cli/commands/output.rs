//! F04–F06, E01, E05–E07, V01 and verified properties: what bead prints and the files it writes for
//! others to read. Files written into a checkout follow the caller's umask while the store stays
//! private, the human output escapes what a terminal would act on, `--json` is always JSON and
//! keeps text exactly, an export never holds a number JSON cannot, a closed stream leaves the
//! command's own status, and the help pages create no state.

use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use beadroll_test_support::{Machine, Run, code, parse_json, printed_id};

use super::fixtures::{machine, metas, strict_json};

/// The permission bits of `path`: `stat.S_IMODE(path.stat().st_mode)`.
fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

/// `output` as a [`Run`]; output that is not UTF-8 fails the test.
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
    // Our copy of the pipe's write end goes with the command.
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
fn checkout_files_stay_readable_in_a_shared_tree() {
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
fn human_output_escapes_tab_and_line_separators() {
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
    // escaped, not dropped: the title is all there, as a reader can see it
    assert!(
        created
            .stdout
            .contains(r"col\x09FORGED\u2028line\u202etitle"),
        "{:?}",
        created.stdout
    );
}

/// F06: a closed stderr on a failed command exits with the command status, not 120.
#[test]
fn failed_command_with_closed_stderr_keeps_its_status() {
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
    let key = printed_id(machine.bead(["create", "pipe"]));
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
    let open_id = printed_id(machine.bead(["create", "open"]));
    machine.bead(["create", "blocked", "--deps", &format!("blocks:{open_id}")]);
    let claimed = printed_id(machine.bead(["create", "claimed"]));
    machine.bead(["claim", &claimed, "--offline"]);
    let deferred = printed_id(machine.bead(["create", "deferred"]));
    machine.bead(["update", &deferred, "--status", "deferred"]);
    let closed = printed_id(machine.bead(["create", "closed"]));
    machine.bead(["close", &closed, "--reason", "done"]);
    let epic = printed_id(machine.bead(["create", "epic", "--type", "epic"]));
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

/// E01: untrusted fields must not execute terminal protocols or forge lines.
#[test]
fn human_output_escapes_terminal_controls() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e01-terminal");
    let title = "safe\x1b]52;c;SGVsbG8=\x07\nFORGED: success";
    let created = machine.run(["create", title]);
    assert_eq!(created.returncode(), 0, "{}", created.stderr);
    let listing = machine.run(["list"]);
    assert_eq!(listing.returncode(), 0, "{}", listing.stderr);
    for output in [&created.stdout, &listing.stdout] {
        assert!(!output.contains('\x1b'), "{output:?}");
        assert!(!output.contains('\x07'), "{output:?}");
        assert!(!output.contains("\nFORGED: success"), "{output:?}");
    }
}

/// E05: every successful --json command must emit valid JSON.
#[test]
fn silent_create_still_honors_json_mode() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e05-silent");
    let created = machine.run(["create", "machine result", "--silent", "--json"]);
    assert_eq!(created.returncode(), 0, "{}", created.stderr);
    let result = parse_json(&created.stdout).unwrap_or_else(|error| {
        panic!(
            "successful --json output was not JSON: {error}; bytes={:?}",
            created.stdout
        )
    });
    let members = result.as_object().unwrap_or_else(|| panic!("{result:?}"));
    assert!(members.contains_key("id"), "{result:?}");
}

/// E06: accepted data must not make Beads JSONL violate RFC 8259.
#[test]
fn an_import_holding_nan_is_refused_whole_with_a_strict_json_error() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e06-json");
    let source = sandbox.root().join("nonfinite.jsonl");
    let nonfinite = concat!(
        r#"{"id":"ex-nan","title":"bad number","status":"open","priority":2,"#,
        r#""issue_type":"task","created_at":"2026-01-01T00:00:00Z","#,
        r#""updated_at":"2026-01-01T00:00:00Z","created_by":"seed","score":NaN}"#,
        "\n",
    );
    std::fs::write(&source, nonfinite).unwrap();
    let imported = machine.run(["import", &source.display().to_string(), "--json"]);
    // refused whole, with an error that is itself strict JSON: nothing is there to export
    assert_eq!(imported.returncode(), 1, "{}", imported.stdout);
    strict_json(&imported.stderr);
    assert_eq!(code(&imported), "VALIDATION", "{}", imported.stderr);
    let exported = machine.run(["export"]);
    assert_eq!(exported.returncode(), 0, "{}", exported.stderr);
    assert_eq!(exported.stdout, "");
}

/// E07: an early pipeline consumer exit must not produce flush errors.
#[test]
fn closed_downstream_pipe_has_defined_quiet_exit() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "e07-pipe");
    let key = printed_id(machine.bead(["create", "pipe target"]));
    let mut command = machine.cmd(["show", &key]).command();
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bead starts");
    // the reader of standard output is gone before bead writes to it
    drop(child.stdout.take());
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("bead's standard error")
        .read_to_end(&mut stderr)
        .expect("bead's standard error");
    let run = Run {
        status: child.wait().expect("bead ends"),
        stdout: String::new(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    };
    // a reader that went away is a quiet exit 0, whether bead found it gone or wrote first
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    assert_eq!(run.stderr, "");
}

/// V01: the standard JSON serializer keeps control data inert and recoverable.
#[test]
fn machine_json_preserves_and_escapes_control_data() {
    let sandbox = crate::sandbox();
    let machine = machine(&sandbox, "v01-json-control");
    let title = "safe\x1b]52;c;SGVsbG8=\x07\nline";
    let result = machine.run(["create", title, "--json"]);
    assert_eq!(result.returncode(), 0, "{}", result.stderr);
    assert!(!result.stdout.contains('\x1b'), "{:?}", result.stdout);
    assert_eq!(strict_json(&result.stdout)["title"].as_str(), Some(title));
}

/// `export --json` without `-o` is `USAGE`: its standard output is Beads JSONL, a document an
/// issue, never the one document `--json` promises. With `-o`, the answer is what was written.
#[test]
fn export_with_json_needs_a_file() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "a");
    machine.bead(["init", "--prefix", "ex"]);
    machine.bead(["create", "x"]);
    let refused = machine.run(["export", "--json"]);
    assert_eq!(code(&refused), "USAGE", "{}", refused.stderr);
    assert!(refused.stdout.is_empty(), "{}", refused.stdout);
    let plain = machine.run(["export"]);
    assert_eq!(plain.returncode(), 0, "{}", plain.stderr);
    assert_eq!(plain.stdout.lines().count(), 1);
    let target = sandbox.root().join("out.jsonl");
    let written = machine.run(["export", "-o", target.to_str().unwrap(), "--json"]);
    assert_eq!(
        parse_json(&written.stdout).unwrap()["exported"].as_i64(),
        Some(1),
        "{}",
        written.stderr
    );
}

/// `show`'s text gives every field its `--json` has: an external reference and an estimate too.
#[test]
fn show_text_gives_the_external_ref_and_the_estimate() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "a");
    machine.bead(["init", "--prefix", "ex"]);
    let made = ["create", "x", "--external-ref", "gh-12", "--estimate", "90"];
    let key = machine.bead(made).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let shown = machine.run(["show", key.as_str()]);
    assert!(
        shown.stdout.contains("external_ref: gh-12"),
        "{}",
        shown.stdout
    );
    assert!(
        shown.stdout.contains("estimated_minutes: 90"),
        "{}",
        shown.stdout
    );
}
