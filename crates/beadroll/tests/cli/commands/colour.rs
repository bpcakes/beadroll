//! Colour (roll-dih8.23): where it is asked for, or a terminal shows it, and never in JSON or
//! instead of escaping what an issue says. Terminals are `hardening::g59` and `hardening::v8`'s.

use beadroll_test_support::{Machine, Run, parse_json};

/// Machine a with tracker `ex` and one issue whose title holds a colour code of its own.
fn set_up(sandbox: &beadroll_test_support::Sandbox) -> (Machine, String) {
    let a = Machine::new(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let made = a.bead(["create", "esc\u{1b}[31mRED"]).unwrap();
    (a, made["id"].as_str().unwrap().to_owned())
}

/// Whether `text` holds an escape character.
fn coloured(text: &str) -> bool {
    text.contains('\u{1b}')
}

/// `text` without bead's colour codes (`ESC [`, digits and semicolons, `m`).
fn uncoloured(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("\u{1b}[") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let end = after
            .find(|c: char| !(c.is_ascii_digit() || c == ';'))
            .unwrap_or(after.len());
        assert!(
            after[end..].starts_with('m'),
            "not a colour code in {text:?}"
        );
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// `bead ARGS` on `a` with `vars` set: what it printed, having succeeded.
fn ok(a: &Machine, args: &[&str], vars: &[(&str, &str)]) -> Run {
    let mut command = a.cmd(args);
    for (key, value) in vars {
        command = command.env(*key, *value);
    }
    let run = command.run();
    assert_eq!(run.returncode(), 0, "{args:?} {vars:?}: {}", run.stderr);
    run
}

#[test]
fn a_pipe_is_plain_unless_colour_is_asked_for() {
    let sandbox = crate::sandbox();
    let (a, _) = set_up(&sandbox);
    assert!(!coloured(&ok(&a, &["list"], &[]).stdout));
    assert!(coloured(
        &ok(&a, &["list"], &[("CLICOLOR_FORCE", "1")]).stdout
    ));
    assert!(coloured(&ok(&a, &["--color=always", "list"], &[]).stdout));
    assert!(coloured(
        &ok(&a, &["list", "--color", "always"], &[]).stdout
    ));
    // NO_COLOR outweighs CLICOLOR_FORCE, and --color both
    let off = [("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")];
    assert!(!coloured(&ok(&a, &["list"], &off).stdout));
    assert!(coloured(&ok(&a, &["--color=always", "list"], &off).stdout));
    let forced = [("CLICOLOR_FORCE", "1")];
    assert!(!coloured(
        &ok(&a, &["--color=never", "list"], &forced).stdout
    ));
    // FORCE_COLOR is not bead's to follow: agent harnesses set it
    assert!(!coloured(
        &ok(&a, &["list"], &[("FORCE_COLOR", "1")]).stdout
    ));
    let json = ok(&a, &["--json", "--color=always", "list"], &[]);
    assert!(!coloured(&json.stdout), "{}", json.stdout);
    assert!(parse_json(&json.stdout).is_ok());
    let wrong = a.run(["--color=sometimes", "list"]);
    assert_eq!(wrong.returncode(), 2, "{}", wrong.stderr);
    assert!(
        wrong.stderr.contains("auto, always, never"),
        "{}",
        wrong.stderr
    );
}

#[test]
fn colour_never_stands_in_for_escaping() {
    let sandbox = crate::sandbox();
    let (a, key) = set_up(&sandbox);
    for args in [&["show", key.as_str()][..], &["list"], &["log"]] {
        let all: Vec<&str> = ["--color=always"].iter().chain(args).copied().collect();
        let shown = ok(&a, &all, &[]).stdout;
        assert!(coloured(&shown), "{args:?}: {shown}");
        let plain = uncoloured(&shown);
        assert!(
            !coloured(&plain),
            "{args:?} lets a raw escape through: {plain:?}"
        );
        assert_eq!(plain, ok(&a, args, &[]).stdout, "{args:?}");
    }
    assert!(
        uncoloured(&ok(&a, &["--color=always", "list"], &[]).stdout).contains("esc\\x1b[31mRED")
    );
    let refused = a.run(["--color=always", "show", "ex-nope"]);
    assert_eq!(refused.returncode(), 1);
    assert!(coloured(&refused.stderr), "{}", refused.stderr);
    assert_eq!(
        uncoloured(&refused.stderr).trim_end(),
        "Error: Issue not found: ex-nope"
    );
}
