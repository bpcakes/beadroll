//! G49: a `meta.json` whose remote is not a string, or a `replica.json` that is not an object, is
//! answered with a structured error or a result, never a crash.

/// G49: F13 shape-checks meta.json and replica.json only partly. A non-string `remote` in
/// meta.json reaches re.fullmatch in Workspace.remote (TypeError), and a replica.json that is
/// valid JSON but not an object reaches own.get in machine_aliases (AttributeError): both end in
/// tracebacks.
mod g49_local_state_shapes_traceback {
    use beadroll_test_support::{Machine, Run, Sandbox, parse_json};

    /// The class's `setUp`: machine a, its tracker initialised with the prefix `ex` and published,
    /// and the key of the one issue it holds.
    fn setup(sandbox: &Sandbox) -> (Machine, String) {
        let a = Machine::new(sandbox, "a");
        a.bead(["init", "--prefix", "ex"]);
        let created = a.bead(["create", "x"]).expect("create prints the issue");
        let key = created["id"].as_str().expect("the issue's id").to_owned();
        a.bead(["sync"]);
        (a, key)
    }

    /// `_g49_answered`: no crash; the command either succeeded or gave the structured JSON error
    /// envelope, with a code.
    fn answered(proc: &Run, what: &str) {
        assert!(!proc.crashed(), "{what}: {}", proc.stderr);
        if proc.returncode() != 0 {
            // `proc.stderr[-300:]`: its last 300 characters
            let start = proc.stderr.char_indices().rev().nth(299);
            let tail = &proc.stderr[start.map_or(0, |(at, _)| at)..];
            let document = parse_json(&proc.stderr).ok();
            let error = document.as_ref().and_then(|d| d.get("error"));
            let error = error.unwrap_or_else(|| {
                panic!(
                    "{what}: exit {} without a structured error: {tail}",
                    proc.returncode()
                )
            });
            let code = error.get("code").and_then(|code| code.as_str());
            assert!(code.is_some(), "{what}: {}", proc.stderr);
        }
    }

    /// G49: meta.json {"remote": 5} (or a list, true, an object) makes info, sync, claim, release
    /// and an owner's close (after the close is recorded) end in a TypeError traceback.
    #[test]
    fn non_string_meta_remote_is_not_a_traceback() {
        let sandbox = crate::sandbox();
        let (a, key) = setup(&sandbox);
        let meta = a.store().join("meta.json");
        let saved = std::fs::read_to_string(&meta).unwrap();
        let bodies = [
            r#"{"remote": 5}"#,
            r#"{"remote": ["x"]}"#,
            r#"{"remote": true}"#,
            r#"{"remote": {"a": 1}}"#,
        ];
        let commands: [&[&str]; 4] = [&["info"], &["sync"], &["claim", &key], &["release", &key]];
        for body in bodies {
            std::fs::write(&meta, body).unwrap();
            for argv in commands {
                let proc = a.run(argv.iter().chain(&["--json"]));
                answered(&proc, &format!("meta={body} {argv:?}"));
            }
        }
        std::fs::write(&meta, &saved).unwrap();
        a.bead(["claim", &key]);
        std::fs::write(&meta, r#"{"remote": 5}"#).unwrap();
        // the close is written locally first, then published
        let closed = a.run(["close", &key]);
        std::fs::write(&meta, &saved).unwrap();
        answered(&closed, "owner close with a non-string meta remote");
        assert_eq!(
            closed.returncode(),
            0,
            "a close that was recorded must not be reported as failed: {}",
            closed.stderr
        );
        assert!(
            closed.stdout.contains("not yet published"),
            "{}",
            closed.stdout
        );
        assert_eq!(a.show(&key)["status"].as_str(), Some("closed"));
    }

    /// G49: replica.json holding a JSON list, number, null or string makes claims, machines and
    /// log end in an AttributeError traceback.
    #[test]
    fn replica_json_that_is_not_an_object_is_not_a_traceback() {
        let sandbox = crate::sandbox();
        let (a, key) = setup(&sandbox);
        a.bead(["claim", &key]);
        let replica = a.home().join("replica.json");
        let saved = std::fs::read_to_string(&replica).unwrap();
        for body in [r#"["x"]"#, "5", "null", r#""s""#] {
            std::fs::write(&replica, body).unwrap();
            for argv in [["claims"], ["machines"], ["log"]] {
                let proc = a.run(argv.iter().chain(&["--json"]));
                answered(&proc, &format!("replica={body} {argv:?}"));
            }
        }
        // The `finally`: should an assertion fail, the sandbox goes with what it holds.
        std::fs::write(&replica, saved).unwrap();
    }
}
