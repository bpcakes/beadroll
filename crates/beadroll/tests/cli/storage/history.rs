//! Cached history keeps filters and aliases current and recovers interrupted cache writes.

use beadroll_test_support::Machine;

fn seed(machine: &Machine) -> String {
    machine.bead(["init", "--prefix", "ex"]);
    let made = machine.bead(["create", "first"]).unwrap();
    let key = made["id"].as_str().unwrap().to_owned();
    machine.bead(["create", "second"]);
    machine.bead([
        "--actor", "named", "update", &key, "--title", "renamed", "--notes", "note",
    ]);
    machine.bead(["comments", "add", &key, "a comment"]);
    key
}

#[test]
fn history_cache_preserves_limits_filters_and_current_machine_aliases() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    let key = seed(&a);
    for args in [
        vec!["log", "--limit", "0"],
        vec!["log", "--limit", "1"],
        vec!["log", "--limit", "-2"],
        vec!["log", &key],
        vec!["log", &key, "--limit", "1"],
    ] {
        for format in [None, Some("--json")] {
            let args: Vec<_> = args.iter().copied().chain(format).collect();
            let _ = std::fs::remove_file(a.store().join("history"));
            let cold = a.run(&args);
            let warm = a.run(&args);
            assert_eq!(cold.returncode(), 0, "{}", cold.stderr);
            assert_eq!(warm.returncode(), 0, "{}", warm.stderr);
            assert_eq!(cold.stdout, warm.stdout, "{args:?}");
        }
    }
    let rows = a.bead(["machines"]).unwrap();
    let machine = rows.as_array().unwrap()[0]["machine"].as_str().unwrap();
    a.bead(["machines", "--name", &format!("{machine}=new-alias")]);
    let rows = a.bead(["log", "--limit", "1"]).unwrap();
    assert_eq!(
        rows.as_array().unwrap()[0]["machine"].as_str(),
        Some("new-alias")
    );
}

#[test]
fn a_killed_or_failed_history_cache_write_keeps_the_journal_and_recovers() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    seed(&a);
    let original = a.run(["log", "--limit", "0", "--json"]);
    for fault in [
        "atomic-write:history=exit(98)",
        "atomic-written:history=exit(99)",
        "atomic-write:history=errno(ENOSPC)",
    ] {
        std::fs::remove_file(a.store().join("history")).unwrap();
        let run = a
            .cmd(["log", "--limit", "0", "--json"])
            .env("BEAD_FAILPOINTS", fault)
            .run();
        if fault.contains("exit(98)") {
            assert_eq!(run.returncode(), 98);
        } else if fault.contains("exit(99)") {
            assert_eq!(run.returncode(), 99);
        } else {
            assert_eq!(run.returncode(), 0);
            assert_eq!(run.stdout, original.stdout);
        }
        let recovered = a.run(["log", "--limit", "0", "--json"]);
        assert_eq!(recovered.returncode(), 0, "{}", recovered.stderr);
        assert_eq!(recovered.stdout, original.stdout);
        assert!(!std::fs::read_dir(a.store()).unwrap().any(|e| {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            name.starts_with(".history.") && name.ends_with(".tmp")
        }));
    }
}
