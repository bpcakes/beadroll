//! `HelpAndPrime`: the overview, one command's help, `prime` and `onboard`, with and without a
//! tracker.

use std::path::Path;

use beadroll_test_support::{Json, Machine, Sandbox, code};

/// `setUp`: a sandbox and one machine, `help-a`.
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
    let overview = Json::parse(&a.run(["--json"]).stdout).unwrap();
    assert!(overview.get("help").is_some(), "{overview:?}");
}

#[test]
fn help_command_shows_one_commands_usage() {
    let (_sandbox, a) = set_up();
    let result = a.run(["help", "claim"]);
    assert_eq!(result.returncode(), 0, "{}", result.stderr);
    assert!(
        result.stdout.contains("usage: bead claim"),
        "{}",
        result.stdout
    );
    assert!(result.stdout.contains("--offline"), "{}", result.stdout);
    // Multi-line texts are lines, not one escaped line.
    assert!(!result.stdout.contains("\\x0a"), "{}", result.stdout);
    assert!(a.run(["prime"]).stdout.contains("\n1. Find work"));
    assert_eq!(code(&a.run(["help", "nonsense", "--json"])), "USAGE");
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
