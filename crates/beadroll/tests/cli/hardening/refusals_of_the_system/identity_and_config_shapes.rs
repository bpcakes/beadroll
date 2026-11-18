//! What `replica.json` and `.beadroll.json` hold is checked; members
//! bead does not know are let be, and a ref or a workspace name it cannot use is `CONFIG`, as is a
//! numeric setting it cannot read, even for `--version`.

use beadroll_test_support::{Json, Machine, code, failure_code, json, json_line};

use super::replica_id;

/// Verified (V6): replica.json and .beadroll.json shapes are validated. The review measured
/// replica.json with an integer alias or extra keys: create, machines and claims exit 0. The
/// refs refs/heads/x, refs/beads/../x, refs/beads/x.lock and 'refs/beads/a b' are CONFIG; the
/// workspaces '.', '..', 101 characters, 'wö' and 'w1\n' are CONFIG; unknown keys are
/// accepted.
#[test]
fn identity_and_tracked_config_shapes_are_validated() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    let replica = a.home().join("replica.json");
    let ident = replica_id(&a);
    let identities = [
        json!({"id": (ident.clone()), "alias": 5}),
        json!({"id": ident, "alias": "m", "extra": [1]}),
    ];
    for body in &identities {
        std::fs::write(&replica, json_line(body)).unwrap();
        let commands: [&[&str]; 3] = [&["create", "y"], &["machines"], &["claims"]];
        for argv in commands {
            let proc = a.run(argv.iter().chain(&["--json"]));
            assert_eq!(
                proc.returncode(),
                0,
                "{} with {}: {}",
                argv[0],
                json_line(body),
                proc.stderr
            );
        }
    }
    let config = a.checkout().join(".beadroll.json");
    let refs = [
        "refs/heads/x",
        "refs/beads/../x",
        "refs/beads/x.lock",
        "refs/beads/a b",
    ];
    let long = "a".repeat(101);
    let workspaces = [".", "..", long.as_str(), "wö", "w1\n"];
    let mut bad: Vec<Json> = refs
        .iter()
        .map(|r| json!({"workspace": "w", "ref": (*r)}))
        .collect();
    bad.extend(workspaces.iter().map(|w| json!({"workspace": (*w)})));
    for body in &bad {
        std::fs::write(&config, json_line(body)).unwrap();
        let proc = a.run(["info", "--json"]);
        assert!(!proc.crashed(), "{}: {}", json_line(body), proc.stderr);
        assert_eq!(
            failure_code(&proc).as_deref(),
            Some("CONFIG"),
            "{}: {}",
            json_line(body),
            proc.stderr
        );
    }
    let future = json!({"workspace": "w", "future": {"x": 1}});
    std::fs::write(&config, json_line(&future)).unwrap();
    let proc = a.run(["info", "--json"]);
    assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
}

/// A numeric setting bead cannot read is `CONFIG` before anything runs, even for `--version`.
#[test]
fn an_unreadable_numeric_setting_refuses_even_version() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "a");
    for flag in ["--version", "-V"] {
        let refused = machine
            .cmd([flag, "--json"])
            .env("BEAD_NET_TIMEOUT", "soon")
            .run();
        assert_eq!(code(&refused), "CONFIG", "{flag}: {}", refused.stderr);
        assert!(
            refused.stderr.contains("BEAD_NET_TIMEOUT"),
            "{}",
            refused.stderr
        );
        let fine = machine.run([flag]);
        assert_eq!(fine.returncode(), 0, "{flag}: {}", fine.stderr);
    }
}

/// A `.beadroll.json` that cannot be read, a directory here, is `IO` naming it, not a file whose
/// JSON is wrong (`CONFIG`), which a file that does not parse still is.
#[test]
fn an_unreadable_tracked_file_is_io_not_bad_json() {
    let sandbox = crate::sandbox();
    let machine = Machine::new(&sandbox, "a");
    let tracked = machine.checkout().join(".beadroll.json");
    std::fs::create_dir(&tracked).expect("a directory where the file goes");
    let refused = machine.run(["info", "--json"]);
    assert_eq!(code(&refused), "IO", "{}", refused.stderr);
    assert!(
        refused.stderr.contains(".beadroll.json"),
        "{}",
        refused.stderr
    );
    std::fs::remove_dir(&tracked).expect("the directory gone");
    std::fs::write(&tracked, "{").expect("a file that does not parse");
    let refused = machine.run(["info", "--json"]);
    assert_eq!(code(&refused), "CONFIG", "{}", refused.stderr);
}
