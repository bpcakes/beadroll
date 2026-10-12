//! G51: a tracked `.beadroll.json` that bead cannot use (a looping or a dangling link, an empty
//! workspace) is a `CONFIG` error, never taken for an absent one, which would quietly move the
//! checkout to the workspace derived from origin.

use beadroll_test_support::{Run, parse_json};

/// A failed JSON call's error code; `TRACEBACK` for a crash (traceback, panic or fatal signal),
/// or `None` for neither.
fn g51_code(run: &Run) -> Option<String> {
    let document = parse_json(&run.stderr).ok();
    let code = document.and_then(|document| document["error"]["code"].as_str().map(str::to_owned));
    code.or_else(|| run.crashed().then(|| "TRACEBACK".to_owned()))
}

/// The first `count` Unicode scalar values of `text`.
fn head(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

mod g51_unusable_tracked_config_is_an_error {
    use std::path::{Path, PathBuf};

    use beadroll_test_support::{Machine, Sandbox, git, parse_json};

    use super::{g51_code, head};

    /// Machine `g51`, whose checkout tracks `{"workspace": "w-tracked"}` in
    /// `.beadroll.json`, with a tracker (prefix `ex`) holding one issue; the config's path.
    fn set_up(sandbox: &Sandbox) -> (Machine, PathBuf) {
        let a = Machine::new(sandbox, "g51");
        let config = a.checkout().join(".beadroll.json");
        std::fs::write(&config, r#"{"workspace": "w-tracked"}"#).unwrap();
        // local: origin must hold no tracker at all for these tests
        a.bead(["init", "--prefix", "ex", "--offline"]);
        a.bead(["create", "tracked issue", "--offline"]);
        (a, config)
    }

    /// `_break`: `.beadroll.json` becomes a link to itself (`loop`) or to a file that is not there.
    fn break_config(config: &Path, kind: &str) {
        std::fs::remove_file(config).unwrap();
        let target = if kind == "loop" {
            ".beadroll.json"
        } else {
            "missing-config.json"
        };
        std::os::unix::fs::symlink(target, config).unwrap();
    }

    /// G51: a `.beadroll.json` that is a looping or a dangling symlink is a `CONFIG` error for
    /// `info`, `list` and `sync`; it is not treated as absent, which would silently make the
    /// workspace the origin-derived one.
    #[test]
    fn unresolvable_config_link_is_config_not_absent() {
        let sandbox = crate::sandbox();
        let (a, config) = set_up(&sandbox);
        for kind in ["loop", "dangling"] {
            break_config(&config, kind);
            for command in ["info", "list", "sync"] {
                let proc = a.run([command, "--json"]);
                let found = (proc.returncode() == 0)
                    .then(|| parse_json(&proc.stdout).ok()?.get("workspace").cloned())
                    .flatten();
                assert_eq!(
                    (proc.returncode(), g51_code(&proc).as_deref()),
                    (1, Some("CONFIG")),
                    "{command} with a {kind} .beadroll.json link: rc={} workspace={found:?} {}",
                    proc.returncode(),
                    head(proc.stderr.trim(), 200)
                );
            }
        }
    }

    /// G51: under a dangling `.beadroll.json` link, following the hint bead gives (`bead init`),
    /// then creating an issue and syncing, does not start a second tracker under the
    /// origin-derived id and publish it to origin.
    #[test]
    fn following_the_hint_under_a_dangling_link_does_not_start_a_second_tracker() {
        let sandbox = crate::sandbox();
        let (a, config) = set_up(&sandbox);
        break_config(&config, "dangling");
        a.run(["init", "--prefix", "ex", "--json"]);
        a.run(["create", "written after the link broke", "--json"]);
        a.run(["sync", "--json"]);
        let published = git(
            ["for-each-ref", "--format=%(refname)", "refs/beads"],
            Some(sandbox.origin()),
        );
        assert_eq!(
            published.trim(),
            "",
            "a second tracker was created and published under the origin-derived workspace"
        );
    }

    /// G51: `"workspace": ""` in `.beadroll.json` is refused with `CONFIG`; it is not accepted as
    /// silently meaning the origin-derived workspace.
    #[test]
    fn empty_workspace_is_refused() {
        let sandbox = crate::sandbox();
        let _set_up = set_up(&sandbox);
        let b = Machine::new(&sandbox, "g51-empty");
        std::fs::write(b.checkout().join(".beadroll.json"), r#"{"workspace": ""}"#).unwrap();
        let proc = b.run(["info", "--json"]);
        let found = (proc.returncode() == 0)
            .then(|| {
                parse_json(&proc.stdout)
                    .ok()?
                    .get("workspace_source")
                    .cloned()
            })
            .flatten();
        assert_eq!(
            (proc.returncode(), g51_code(&proc).as_deref()),
            (1, Some("CONFIG")),
            "an empty tracked workspace was accepted (workspace_source={found:?})"
        );
    }
}
