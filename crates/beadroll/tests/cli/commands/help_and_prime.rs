//! The overview, one command's help, `prime` and `onboard`, with and without a
//! tracker.

use std::path::Path;
use std::process::Command;

use beadroll_test_support::{Machine, Run, Sandbox, code, parse_json};

/// A sandbox and one machine, `help-a`.
fn set_up() -> (Sandbox, Machine) {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "help-a");
    (sandbox, a)
}

#[test]
fn no_arguments_and_help_print_the_overview_and_exit_zero() {
    let (_sandbox, a) = set_up();
    for argv in [&[][..], &["--help"], &["-h"], &["help"]] {
        let result = a.run(argv);
        assert_eq!(result.returncode(), 0, "{argv:?}: {}", result.stderr);
        for section in ["Find work", "Do work", "Coordinate", "Agents", "--json"] {
            assert!(
                result.stdout.contains(section),
                "{argv:?}: no {section:?} in {}",
                result.stdout
            );
        }
        assert!(!result.crashed(), "{argv:?}: {}", result.stderr);
    }
    let overview = parse_json(&a.run(["--json"]).stdout).unwrap();
    assert!(overview.get("help").is_some(), "{overview:?}");
}

#[test]
fn help_command_shows_one_commands_usage() {
    let (_sandbox, a) = set_up();
    let result = a.run(["help", "claim"]);
    assert_eq!(result.returncode(), 0, "{}", result.stderr);
    assert!(
        result.stdout.contains("Usage: bead claim"),
        "{}",
        result.stdout
    );
    assert!(result.stdout.contains("--offline"), "{}", result.stdout);
    // Multi-line texts are lines, not one escaped line.
    assert!(!result.stdout.contains("\\x0a"), "{}", result.stdout);
    assert!(a.run(["prime"]).stdout.contains("\n1. Find work"));
    assert_eq!(code(&a.run(["help", "nonsense", "--json"])), "USAGE");
}

/// The commands, in help display order.
const COMMANDS: [&str; 28] = [
    "init", "import", "export", "create", "update", "claim", "release", "takeover", "claims",
    "close", "reopen", "delete", "show", "list", "ready", "blocked", "search", "dep", "comments",
    "lint", "sync", "log", "info", "machines", "recap", "help", "prime", "onboard",
];

/// Each command answers both `NAME --help` and `help NAME` with its usage, and a name
/// that is no command is a usage error that suggests the one meant. The unit test in
/// `src/cli/spec.rs` checks the command table itself.
#[test]
fn every_command_has_help() {
    let (_sandbox, a) = set_up();
    for name in COMMANDS {
        for argv in [[name, "--help"], ["help", name]] {
            let result = a.run(argv);
            assert_eq!(result.returncode(), 0, "{argv:?}: {}", result.stderr);
            let (stdout, usage) = (&result.stdout, format!("Usage: bead {name}"));
            assert!(stdout.contains(&usage), "{argv:?}: {stdout}");
        }
    }
    let refused = a.run(["lisst", "--json"]);
    assert_eq!(code(&refused), "USAGE", "{}", refused.stderr);
    let error = parse_json(&refused.stderr).unwrap();
    let message = error["error"]["message"].as_str().expect("a message");
    assert!(message.contains("'list'"), "{message}");
}

/// `bead prime` names every error code bead can answer with, as the onboarding text promises: a
/// code added later fails this until prime says what it means.
#[test]
fn prime_names_every_error_code() {
    let (_sandbox, a) = set_up();
    let primed = a.run(["prime"]);
    assert_eq!(primed.returncode(), 0, "{}", primed.stderr);
    let words: std::collections::BTreeSet<&str> = (primed.stdout)
        .split(|c: char| !(c.is_ascii_uppercase() || c == '_'))
        .collect();
    let missing: Vec<&str> = (beadroll_core::code::Code::ALL.iter())
        .map(|code| code.as_str())
        .filter(|code| !words.contains(code))
        .collect();
    assert_eq!(missing, Vec::<&str>::new(), "prime does not name these");
}

#[test]
fn prime_and_onboard_work_with_and_without_a_tracker() {
    let (sandbox, a) = set_up();
    let outside = a.cmd(["prime"]).cwd(sandbox.root()).run();
    assert_eq!(outside.returncode(), 0, "{}", outside.stderr);
    assert!(
        outside.stdout.contains("Not inside a git repository"),
        "{}",
        outside.stdout
    );
    let before = a.run(["prime"]);
    assert_eq!(before.returncode(), 0, "{}", before.stderr);
    assert!(
        before.stdout.contains("No tracker here yet"),
        "{}",
        before.stdout
    );
    // onboard too, before there is a tracker, as its name says
    let onboard = a.run(["onboard"]);
    assert_eq!(onboard.returncode(), 0, "{}", onboard.stderr);
    assert!(!onboard.stdout.trim().is_empty(), "onboard printed nothing");
    a.bead(["init", "--prefix", "ex"]);
    let created = a.bead(["create", "primed"]).unwrap();
    let key = created["id"].as_str().unwrap();
    a.bead(["claim", key, "--offline"]);
    let primed = a.bead(["prime"]).unwrap();
    let prime = primed["prime"].as_str().unwrap();
    let actor = primed["actor"].as_str().unwrap();
    for wanted in ["bead claim ID", "ALREADY_CLAIMED", "Tracker 'ex'", actor] {
        assert!(prime.contains(wanted), "no {wanted:?} in {prime}");
    }
    assert!(actor.contains("help-a-thread-0001"), "{actor}");
    let onboard = a.run(["onboard"]);
    assert_eq!(onboard.returncode(), 0, "{}", onboard.stderr);
    assert!(onboard.stdout.contains("bead prime"), "{}", onboard.stdout);
    assert!(onboard.stdout.contains("bead ready"), "{}", onboard.stdout);
    // This repository dogfoods it: its AGENTS.md, found from the source tree, not from the binary.
    let agents = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../AGENTS.md");
    let agents = std::fs::read_to_string(agents).unwrap();
    assert!(
        agents.contains(onboard.stdout.trim()),
        "AGENTS.md's work-tracking block must be bead onboard's output"
    );
}

/// V02: top-level and subcommand help are local, side-effect free operations.
#[test]
fn help_needs_no_repository_and_creates_no_state() {
    // The sandbox names the bead under test; the help runs in a new directory of its own, in no
    // repository, with the test's whole environment and a home that does not exist.
    let sandbox = crate::sandbox();
    let root = sandbox.root().join("help-outside-a-repository");
    std::fs::create_dir(&root).unwrap();
    let state = root.join("state");
    for args in [&["--help"][..], &["create", "--help"]] {
        let output = Command::new(sandbox.bead())
            .args(args)
            .current_dir(&root)
            .env("BEAD_HOME", &state)
            .output()
            .expect("bead runs");
        let result = Run {
            status: output.status,
            stdout: String::from_utf8(output.stdout).expect("UTF-8 output"),
            stderr: String::from_utf8(output.stderr).expect("UTF-8 errors"),
        };
        assert_eq!(result.returncode(), 0, "{args:?}: {}", result.stderr);
        assert!(
            result.stdout.to_lowercase().contains("usage:"),
            "{args:?}: {}",
            result.stdout
        );
        assert!(!state.exists(), "{args:?} created {}", state.display());
    }
}

/// `prime` says what the reader holds provisionally and what is not yet published, as `claims` and
/// every other command say it: after an offline claim it said `0 held by you` and nothing more.
#[test]
fn prime_says_what_is_held_provisionally_and_not_yet_published() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = a.bead(["create", "x"]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    a.bead(["claim", key.as_str(), "--offline"]);
    let primed = a.run(["prime"]);
    assert_eq!(primed.returncode(), 0, "{}", primed.stderr);
    let text = primed.stdout.replace('\n', " ");
    assert!(
        text.contains("provisionally by you") && text.contains(&key),
        "{text}"
    );
    assert!(
        text.contains("1 event not yet published: bead sync"),
        "{text}"
    );
    a.bead(["sync"]);
    // the workflow below names "not yet published" always: the count is what must have gone
    let primed = a.run(["prime"]);
    assert!(
        !primed.stdout.contains("published: bead sync"),
        "{}",
        primed.stdout
    );
}
