//! `bead close`, `reopen` and `delete` of several issues decide every one from the state before
//! they write and read the state once more after (roll-lqjj.28): one named twice is answered as the
//! event before left it, on a clean journal, one with a newer bead's operation in it, and one with a
//! takeover of an id no event has, which keeps the store's projection from being extended.

use std::path::Path;

use beadroll_core::json::{Format, write};
use beadroll_store::{Clock, uuid7};
use beadroll_test_support::{Json, Machine, json, parse_json};

/// What a journal holds besides this machine's records.
#[derive(Clone, Copy, Debug)]
enum Journal {
    /// Nothing.
    Clean,
    /// An event of an operation this bead does not know, as a newer bead writes.
    Newer,
    /// A takeover of an id no event has: replay refuses it, and a projection of it is not extended.
    Dangling,
}

const JOURNALS: [Journal; 3] = [Journal::Clean, Journal::Newer, Journal::Dangling];

/// Runs `bead ARGS` on `machine`, which must succeed: its standard output.
fn ok(machine: &Machine, args: &[&str]) -> String {
    let run = machine.run(args);
    assert_eq!(run.returncode(), 0, "{}", run.stderr);
    run.stdout
}

/// Creates an issue titled `title` on `machine`, beneath `parent` when given: its key.
fn create(machine: &Machine, title: &str, parent: Option<&str>) -> String {
    let mut args = vec!["create", title];
    args.extend(
        parent
            .map(|parent| ["--parent", parent])
            .into_iter()
            .flatten(),
    );
    machine.bead(&args).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The lines of `out` that answer for an issue.
fn answers(out: &str) -> Vec<&str> {
    out.lines().filter(|line| line.starts_with('✓')).collect()
}

/// Puts `journal`'s record, built on everything `machine` holds, in a journal of a machine of its
/// own, and checks that replay reports what it should of it.
fn add(machine: &Machine, journal: Journal) {
    let event = |op: &str, data: Json| {
        let info = machine.bead(["info"]).unwrap();
        let id = uuid7(&Clock::system());
        json!({"v": 1, "id": (id.as_str()), "replica": "feedface",
               "deps": (info["frontier"].clone()), "actor": {"agent": "newer"},
               "op": op, "issue": (id.as_str()), "data": data})
    };
    let (record, problem) = match journal {
        Journal::Clean => return,
        Journal::Newer => (event("archive", json!({})), "unknown operation 'archive'"),
        Journal::Dangling => (
            event(
                "claim",
                json!({"mode": "online", "takeover": (uuid7(&Clock::system()))}),
            ),
            "takeover names a claim that is not a claim on this issue",
        ),
    };
    let folder = machine.store().join("events").join("feedface");
    std::fs::create_dir_all(&folder).unwrap();
    let line = write(&record, Format::ASCII).unwrap() + "\n";
    std::fs::write(Path::new(&folder).join("000001.jsonl"), line).unwrap();
    let info = machine.bead(["info"]).unwrap();
    let problems: Vec<&str> = (info["integrity"].as_array().unwrap().iter())
        .map(|entry| entry["problem"].as_str().unwrap())
        .collect();
    assert_eq!(problems, [problem]);
}

#[test]
fn several_issues_and_one_named_twice_are_answered_as_each_event_left_them() {
    for journal in JOURNALS {
        let sandbox = crate::sandbox();
        let a = Machine::new(&sandbox, "a");
        a.bead(["init", "--prefix", "ex"]);
        let [one, two, three, four] = ["one", "two", "three", "four"].map(|t| create(&a, t, None));
        let epic = create(&a, "epic", None);
        ok(&a, &["update", &epic, "--type", "epic"]);
        let kid = create(&a, "kid", Some(&epic));
        let grandkid = create(&a, "grandkid", Some(&kid));
        add(&a, journal);
        ok(&a, &["claim", &one]);
        ok(&a, &["claim", &three, "--offline"]);
        assert_eq!(
            a.cmd(["claim", &four]).thread("other").run().returncode(),
            0
        );

        // the claim ended once, the issue named again already closed
        let closed = ok(&a, &["close", &one, &two, &one, "--offline", "-r", "done"]);
        assert_eq!(
            answers(&closed),
            [
                format!("✓ Closed {one}: one"),
                format!("✓ Closed {two}: two"),
                format!("✓ Already closed {one}: one"),
            ],
            "{journal:?}"
        );
        assert!(closed.contains(&format!("{one} was already closed: the reason given")));
        assert!(a.show(&one)["claim"] == Json::Null, "{journal:?}");
        // a delete leaves a provisional claim, so the issue named again is deleted again
        let deleted = a.run(["delete", &three, &three, "--offline", "--json"]);
        let deleted = parse_json(&deleted.stdout).unwrap();
        let unchanged: Vec<bool> = (deleted.as_array().unwrap().iter())
            .map(|item| item.get("unchanged").is_some())
            .collect();
        assert_eq!(unchanged, [false, false], "{journal:?}");
        // another owner's claim, forced: ended by the first close, the second finds it closed
        let forced = ok(&a, &["close", &four, &four, "--force", "--offline"]);
        assert_eq!(
            answers(&forced),
            [
                format!("✓ Closed {four}: four"),
                format!("✓ Already closed {four}: four")
            ],
            "{journal:?}"
        );
        // named and beneath a named one: once, children first
        let recursive = ok(&a, &["close", "--recursive", &epic, &kid, "--offline"]);
        assert_eq!(
            answers(&recursive),
            [
                format!("✓ Closed {grandkid}: grandkid"),
                format!("✓ Closed {kid}: kid"),
                format!("✓ Closed {epic}: epic"),
            ],
            "{journal:?}"
        );
        let reopened = ok(&a, &["reopen", &kid, &one, &kid, "--offline"]);
        assert_eq!(
            answers(&reopened),
            [
                format!("✓ Reopened {kid}: kid"),
                format!("✓ Reopened {one}: one"),
                format!("✓ Already open {kid}: kid"),
            ],
            "{journal:?}"
        );
        for (key, status) in [
            (&one, "open"),
            (&two, "closed"),
            (&three, "tombstone"),
            (&four, "closed"),
            (&epic, "closed"),
            (&kid, "open"),
            (&grandkid, "closed"),
        ] {
            assert_eq!(
                a.show(key)["status"].as_str(),
                Some(status),
                "{journal:?} {key}"
            );
        }
    }
}

#[test]
fn a_transition_of_several_issues_reads_the_state_once_more_however_the_projection_fares() {
    for journal in JOURNALS {
        let sandbox = crate::sandbox();
        let a = crate::rust_only(&sandbox, "a");
        a.bead(["init", "--prefix", "ex"]);
        let keys: Vec<String> = (0..6).map(|n| create(&a, &format!("t{n}"), None)).collect();
        add(&a, journal);
        // The projection read, or the journal: once, and once more after the events only when
        // the projection cannot be extended past them. Every issue's state read again after the
        // event before it would load the journal once an issue.
        let reads = match journal {
            Journal::Dangling => 2,
            Journal::Clean | Journal::Newer => 1,
        };
        let stop = |nth: usize| format!("journal.after-enumerate@nth({nth})=exit(97)");
        for op in ["close", "reopen", "delete"] {
            let mut args = vec![op];
            args.extend(keys.iter().map(String::as_str));
            args.push(keys[0].as_str());
            args.push("--offline");
            // the last read is reached, and stops it before anything is written
            let stopped = a.cmd(&args).env("BEAD_FAILPOINTS", stop(reads)).run();
            assert_eq!(
                stopped.returncode(),
                97,
                "{journal:?} {op}: {}",
                stopped.stderr
            );
            let run = a.cmd(&args).env("BEAD_FAILPOINTS", stop(reads + 1)).run();
            assert_eq!(run.returncode(), 0, "{journal:?} {op}: {}", run.stderr);
            let verb = answers(&run.stdout)
                .iter()
                .filter(|line| !line.contains("Already"))
                .count();
            assert_eq!(verb, keys.len(), "{journal:?} {op}: {}", run.stdout);
        }
    }
}
