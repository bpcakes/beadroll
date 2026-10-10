//! `V6_IdentityAndConfigShapes`: what `replica.json` and `.beadroll.json` hold is checked; members
//! bead does not know are let be, and a ref or a workspace name it cannot use is `CONFIG`.

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, json};

use super::{error_code, replica_id};

/// `body` as `json.dumps(body)` writes it.
fn dumps(body: &Json) -> String {
    write(body, Format::ASCII).expect("JSON Python can write")
}

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
        std::fs::write(&replica, dumps(body)).unwrap();
        let commands: [&[&str]; 3] = [&["create", "y"], &["machines"], &["claims"]];
        for argv in commands {
            let proc = a.run(argv.iter().chain(&["--json"]));
            assert_eq!(
                proc.returncode(),
                0,
                "{} with {}: {}",
                argv[0],
                dumps(body),
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
        std::fs::write(&config, dumps(body)).unwrap();
        let proc = a.run(["info", "--json"]);
        assert!(!proc.crashed(), "{}: {}", dumps(body), proc.stderr);
        assert_eq!(
            error_code(&proc).as_deref(),
            Some("CONFIG"),
            "{}: {}",
            dumps(body),
            proc.stderr
        );
    }
    let future = json!({"workspace": "w", "future": {"x": 1}});
    std::fs::write(&config, dumps(&future)).unwrap();
    let proc = a.run(["info", "--json"]);
    assert_eq!(proc.returncode(), 0, "{}", proc.stderr);
}
