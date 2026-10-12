//! Init acknowledges publication only when its write reached the coordination remote.

use beadroll_test_support::{Machine, git, parse_json};

use super::{commit, text};

#[test]
fn init_acknowledges_publication_but_repeat_and_join_do_not_claim_a_push() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let run = a.run(["init"]);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    let expected = format!(
        "Published to {} (refs/beads/journal)",
        sandbox.origin().display()
    );
    assert!(
        run.stdout.lines().any(|line| line == expected),
        "{}",
        run.stdout
    );
    assert!(
        !git(
            ["ls-remote", text(sandbox.origin()), "refs/beads/journal"],
            None
        )
        .is_empty()
    );

    let repeated = a.run(["init"]);
    assert_eq!(repeated.returncode(), 0, "{}", repeated.stderr);
    assert!(
        !repeated.stdout.contains("Published to"),
        "{}",
        repeated.stdout
    );
    let b = Machine::new(&sandbox, "b");
    let joined = b.run(["init"]);
    assert_eq!(joined.returncode(), 0, "{}", joined.stderr);
    assert!(joined.stdout.contains("joined it"), "{}", joined.stdout);
    assert!(!joined.stdout.contains("Published to"), "{}", joined.stdout);
}

#[test]
fn init_acknowledges_the_chosen_remote_and_ref() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let remote = sandbox.bare("tracker.git");
    commit(&a, &["remote", "add", "tracker", text(&remote)]);
    std::fs::write(
        a.checkout().join(".beadroll.json"),
        r#"{"workspace":"publication-test","ref":"refs/beads/custom"}"#,
    )
    .unwrap();
    let run = a.run(["init", "--remote", "tracker"]);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    let expected = format!("Published to {} (refs/beads/custom)", remote.display());
    assert!(
        run.stdout.lines().any(|line| line == expected),
        "{}",
        run.stdout
    );
    assert!(!git(["ls-remote", text(&remote), "refs/beads/custom"], None).is_empty());
    assert!(git(["ls-remote", text(sandbox.origin()), "refs/beads/*"], None).is_empty());
}

#[test]
fn offline_init_and_init_without_a_remote_do_not_acknowledge_publication() {
    for offline in [true, false] {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "a");
        let args = if offline {
            vec!["init", "--offline"]
        } else {
            commit(&a, &["remote", "remove", "origin"]);
            vec!["init"]
        };
        let run = a.run(args);
        assert_eq!(run.returncode(), 0, "{}", run.stderr);
        assert!(!run.stdout.contains("Published to"), "{}", run.stdout);
        if offline {
            assert!(
                run.stdout.contains("1 event not yet published"),
                "{}",
                run.stdout
            );
        }
        assert!(git(["ls-remote", text(sandbox.origin()), "refs/beads/*"], None).is_empty());
    }
}

#[test]
fn failed_init_publication_keeps_the_warning_without_an_acknowledgment() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    let run = a
        .cmd(["init"])
        .env(
            "BEAD_FAILPOINTS",
            "publish.result@always=error(REMOTE_REFUSED)",
        )
        .run();
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    // This code comes from the injected publish result, proving that the fault fired.
    assert!(
        run.stdout.contains("not yet published (REMOTE_REFUSED)"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("1 event not yet published"),
        "{}",
        run.stdout
    );
    assert!(!run.stdout.contains("Published to"), "{}", run.stdout);
    assert!(git(["ls-remote", text(sandbox.origin()), "refs/beads/*"], None).is_empty());
}

#[test]
fn init_json_remains_a_single_document_without_human_acknowledgments() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let run = a.run(["init", "--json"]);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    assert!(run.stderr.is_empty(), "{}", run.stderr);
    let result = parse_json(&run.stdout).expect("one JSON document");
    assert_eq!(result["imported"].as_i64(), Some(0));
    assert!(result.get("unpublished").is_none(), "{result:?}");
    assert!(!run.stdout.contains("Published to"), "{}", run.stdout);
}
