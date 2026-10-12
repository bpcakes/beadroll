//! `bead log` says who wrote each event as its claims name them, and what a `set` changed.

use beadroll_test_support::{Json, Machine};

/// The words of the human `bead log KEY` row for each event, in order.
fn rows(machine: &Machine, key: &str) -> Vec<Vec<String>> {
    let run = machine.run(["log", key]);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    run.stdout
        .lines()
        .map(|line| line.split_whitespace().map(str::to_owned).collect())
        .collect()
}

#[test]
fn log_names_a_named_actor_and_the_fields_a_set_changed() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let made = a.bead(["create", "Named"]).unwrap();
    let key = made["id"].as_str().unwrap().to_owned();
    let renamed = a.run(["--actor", "alice", "update", &key, "--title", "Renamed"]);
    assert_eq!(renamed.returncode(), 0, "{}", renamed.stderr);
    let noted = a
        .cmd(["update", &key, "--notes", "n", "-p", "1"])
        .env("BEAD_ACTOR", "bob")
        .run();
    assert_eq!(noted.returncode(), 0, "{}", noted.stderr);

    // time, machine, who, operation, issue, branch
    let rows = rows(&a, &key);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0][2..6], ["codex", "a-thread", "create", key.as_str()]);
    assert_eq!(rows[1][2..5], ["alice", "set", "title"]);
    assert_eq!(rows[2][2..5], ["bob", "set", "notes,priority"]);

    let log = a.bead(["log", &key]).unwrap();
    let column = |name: &str| -> Vec<Json> {
        log.as_array()
            .unwrap()
            .iter()
            .map(|row| row[name].clone())
            .collect()
    };
    let names = column("name");
    assert_eq!(names[0], Json::Null);
    assert_eq!(names[1].as_str(), Some("alice"));
    assert_eq!(names[2].as_str(), Some("bob"));
    // the agent and thread stay as the event records them
    assert!(
        column("agent")
            .iter()
            .all(|agent| agent.as_str() == Some("codex"))
    );
    assert!(
        column("op")
            .iter()
            .skip(1)
            .all(|op| op.as_str() == Some("set"))
    );
}
