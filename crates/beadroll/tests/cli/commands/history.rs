//! `bead log` says who wrote each event as its claims name them, and what a `set` changed; `bead
//! info` and `bead machines` count the journal's events as its files hold them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use beadroll_core::id::EventId;
use beadroll_core::json::{Format, Object, write};
use beadroll_core::time::event_time;
use beadroll_test_support::{Json, Machine, Sandbox, json, parse_json};

use super::bp_test::segments;

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

/// What a store's files hold, read as the test reads them: each machine's events in its journal,
/// by id, every event's causes, an attempt file's among them, and how many segments there are.
struct Files {
    journals: BTreeMap<String, BTreeSet<String>>,
    causes: BTreeMap<String, Vec<String>>,
    segments: usize,
}

fn files(store: &Path) -> Files {
    let mut found = Files {
        journals: BTreeMap::new(),
        causes: BTreeMap::new(),
        segments: 0,
    };
    let add = |found: &mut Files, event: &Json| -> String {
        let id = event["id"].as_str().expect("an event id").to_owned();
        let causes = event["deps"].as_array().unwrap_or_default().to_vec();
        let causes = causes.iter().map(|c| c.as_str().unwrap().to_owned());
        found.causes.insert(id.clone(), causes.collect());
        id
    };
    for segment in segments(store) {
        found.segments += 1;
        let machine = segment.parent().unwrap().file_name().unwrap();
        let machine = machine.to_str().unwrap().to_owned();
        for line in std::fs::read_to_string(&segment).unwrap().lines() {
            // a line that is not an event is an integrity problem, and no event
            let Ok(event) = parse_json(line) else {
                continue;
            };
            let id = add(&mut found, &event);
            found
                .journals
                .entry(machine.clone())
                .or_default()
                .insert(id);
        }
    }
    for folder in std::fs::read_dir(store.join("events")).unwrap() {
        let claims = folder.unwrap().path().join("claims");
        for file in std::fs::read_dir(claims).into_iter().flatten() {
            let text = std::fs::read_to_string(file.unwrap().path()).unwrap();
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                add(&mut found, &parse_json(line).unwrap());
            }
        }
    }
    found
}

/// What `machine` answers to `log`, `info` and `machines`, as JSON and as lines.
fn histories(machine: &Machine) -> Vec<String> {
    let mut out = Vec::new();
    for args in [
        ["log", "--limit", "0"],
        ["info", "", ""],
        ["machines", "", ""],
    ] {
        let args: Vec<&str> = args.into_iter().filter(|a| !a.is_empty()).collect();
        for json in [true, false] {
            let mut args = args.clone();
            if json {
                args.insert(0, "--json");
            }
            let run = machine.run(&args);
            assert_eq!(run.returncode(), 0, "{args:?}: {}", run.stderr);
            out.push(run.stdout);
        }
    }
    out
}

/// Two machines' tracker on `a`, as `a`'s store holds it: an attempt file, a key both machines
/// imported (one copy renamed), an attempt that awaits the remote, a line of `a`'s journal that is
/// not an event, and a segment of `b`'s gone, the events after it waiting for their causes.
fn troubled(sandbox: &Sandbox) -> Machine {
    let a = Machine::new(sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["sync"]);
    let mut b = Machine::new(sandbox, "b");
    b.extra.insert("BEAD_ROTATE_BYTES".into(), "600".into());
    b.bead(["sync"]);
    // the same record imported on both machines: one copy is renamed in the merge
    let record = json!({
        "id": "ex-a",
        "title": "A",
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "created_by": "seed",
    });
    let export = sandbox.root().join("ext.jsonl");
    std::fs::write(&export, write(&record, Format::ASCII).unwrap() + "\n").unwrap();
    let export = export.to_str().unwrap();
    a.bead(["import", export]);
    b.bead(["import", export]);
    let made = a.bead(["create", "claimed"]).unwrap();
    let x = made["id"].as_str().unwrap().to_owned();
    a.bead(["claim", &x]); // an attempt file
    let y = a.bead(["create", "to claim"]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for machine in [&a, &b, &a] {
        machine.bead(["sync"]);
    }
    for n in 0..6 {
        b.bead(["comments", "add", &x, &format!("from b, {n}")]);
    }
    a.bead(["sync"]);
    // an attempt that reached the remote, its answer lost: awaiting it
    let stopped = a
        .cmd(["claim", &y])
        .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
        .run();
    assert_eq!(stopped.returncode(), 99, "{}", stopped.stderr);
    let store = a.store();
    // a line of a's journal that is not an event, and a segment of b's gone, the events after it
    // waiting for their causes
    let own = segments(&store)
        .into_iter()
        .find(|path| path.parent().unwrap() == store.join("events").join(machine_id(&a)))
        .unwrap();
    let text = std::fs::read_to_string(&own).unwrap();
    std::fs::write(&own, text + "this is not json\n").unwrap();
    let theirs: Vec<_> = (segments(&store).into_iter())
        .filter(|path| path.parent().unwrap() != own.parent().unwrap())
        .collect();
    assert!(theirs.len() > 2, "{theirs:?}");
    std::fs::remove_file(&theirs[1]).unwrap();
    a
}

/// `info` counts every event the journal's files hold, each machine's, the frontier, the segments,
/// an attempt awaiting the remote, the events waiting for their causes, an integrity problem and a
/// renamed key; `machines` each machine's events and when it last wrote; `log` every event that
/// applies. Each answers alike with the store's projection and without it (roll-lqjj.31).
#[test]
fn info_machines_and_log_count_what_the_journals_files_hold() {
    let sandbox = crate::sandbox();
    let a = troubled(&sandbox);
    let store = a.store();
    let held = files(&store);
    let info = a.bead(["info"]).unwrap();
    assert_eq!(info["events"].as_i64(), Some(held.causes.len() as i64));
    let by_machine: Object = (held.journals.iter())
        .map(|(machine, ids)| (machine.clone(), Json::from(ids.len() as i64)))
        .collect();
    assert_eq!(info["events_by_machine"], Json::from(by_machine));
    let named: BTreeSet<&String> = held.causes.values().flatten().collect();
    let frontier: Vec<Json> = (held.causes.keys())
        .filter(|id| !named.contains(id))
        .map(|id| Json::from(id.as_str()))
        .collect();
    assert_eq!(info["frontier"], Json::from(frontier));
    assert_eq!(info["segments"].as_i64(), Some(held.segments as i64));
    assert_eq!(
        info["pending_ownership"].as_array().map(<[Json]>::len),
        Some(1)
    );
    assert_eq!(
        info["integrity"].as_array().map(<[Json]>::len),
        Some(1),
        "{info:?}"
    );
    assert_eq!(info["renamed_keys"].as_array().map(<[Json]>::len), Some(1));
    let waiting = info["waiting_events"].as_i64().unwrap();
    assert!(waiting > 0, "{info:?}");
    // every event applies but the one init and those waiting
    let log = a.bead(["log", "--limit", "0"]).unwrap();
    let rows = log.as_array().unwrap().len() as i64;
    assert_eq!(rows + 1 + waiting, info["events"].as_i64().unwrap());

    let machines = a.bead(["machines"]).unwrap();
    let listed: Vec<(String, i64, String)> = (machines.as_array().unwrap().iter())
        .map(|row| {
            let text = |key: &str| row[key].as_str().unwrap().to_owned();
            (
                text("machine"),
                row["events"].as_i64().unwrap(),
                text("last_event"),
            )
        })
        .collect();
    let expected: Vec<(String, i64, String)> = (held.journals.iter())
        .map(|(machine, ids)| {
            let last = ids
                .iter()
                .filter_map(|id| EventId::parse(id))
                .max()
                .unwrap();
            (machine.clone(), ids.len() as i64, event_time(last.millis()))
        })
        .collect();
    assert_eq!(listed, expected);
    assert_eq!(listed.len(), 2);

    let kept = histories(&a);
    std::fs::remove_file(store.join("projection")).unwrap();
    assert_eq!(histories(&a), kept);
}

/// The id of `machine`'s own journal, as `info` names it.
fn machine_id(machine: &Machine) -> String {
    let info = machine.bead(["info"]).unwrap();
    info["machine"].as_str().unwrap().to_owned()
}
