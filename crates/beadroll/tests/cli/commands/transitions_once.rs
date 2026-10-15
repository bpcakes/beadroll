//! `bead close`, `reopen` and `delete` of several issues decide every one from the state before
//! they write and read the state once more after, and once more before each issue named again
//! (roll-lqjj.28): every issue is answered as the events before it left it, on a clean journal, one
//! with a newer bead's operation in it, and one with a takeover of an id no event has, which keeps
//! the store's projection from being extended; and on journals whose waiting events keep the
//! events written from applying, or from seeing everything.

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

/// Writes `record` as the only one of the journal of a machine of its own, `feedface`.
fn place(machine: &Machine, record: &Json) {
    let folder = machine.store().join("events").join("feedface");
    std::fs::create_dir_all(&folder).unwrap();
    let line = write(record, Format::ASCII).unwrap() + "\n";
    std::fs::write(Path::new(&folder).join("000001.jsonl"), line).unwrap();
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
    place(machine, &record);
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
fn a_transition_reads_the_state_once_more_and_again_only_before_an_issue_named_again() {
    for journal in JOURNALS {
        let sandbox = crate::sandbox();
        let a = crate::rust_only(&sandbox, "a");
        a.bead(["init", "--prefix", "ex"]);
        let keys: Vec<String> = (0..6).map(|n| create(&a, &format!("t{n}"), None)).collect();
        add(&a, journal);
        // The projection read, or the journal: once, and once more after the events only when
        // the projection cannot be extended past them, and before an issue named again, after the
        // events before it, likewise. Every issue's state read again after the event before it
        // would load the journal once an issue.
        let stop = |nth: usize| format!("journal.after-enumerate@nth({nth})=exit(97)");
        for (op, again) in [("close", false), ("reopen", true), ("delete", true)] {
            let reads = match journal {
                Journal::Dangling if again => 3,
                Journal::Dangling => 2,
                Journal::Clean | Journal::Newer => 1,
            };
            let mut args = vec![op];
            args.extend(keys[..3].iter().map(String::as_str));
            if again {
                args.push(keys[0].as_str());
            }
            args.extend(keys[3..].iter().map(String::as_str));
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

#[test]
fn an_issue_named_again_after_an_event_that_waits_is_decided_as_it_was() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let [claimed, open, closed, deleted] =
        ["claimed", "open", "closed", "deleted"].map(|t| create(&a, t, None));
    ok(&a, &["claim", &claimed, "--offline"]);
    ok(&a, &["close", &closed]);
    // On the frontier, an event waiting for a cause no record has: every event written after it
    // builds on it and waits too, so an issue named again is as it was before the first.
    let id = uuid7(&Clock::system());
    place(
        &a,
        &json!({"v": 1, "id": (id.as_str()), "replica": "feedface",
                "deps": [(uuid7(&Clock::system()))], "actor": {"agent": "newer"},
                "op": "set", "issue": (id.as_str()), "data": {"fields": {"title": "x"}}}),
    );
    let waiting = || {
        a.bead(["info"]).unwrap()["waiting_events"]
            .as_i64()
            .unwrap()
    };
    assert_eq!(waiting(), 1);
    let closes = ok(&a, &["close", &claimed, &open, &claimed, "--offline"]);
    assert_eq!(
        answers(&closes),
        [
            format!("✓ Closed {claimed}: claimed"),
            format!("✓ Closed {open}: open"),
            format!("✓ Closed {claimed}: claimed"),
        ]
    );
    assert_eq!(waiting(), 4);
    let reopens = ok(&a, &["reopen", &closed, &closed, "--offline"]);
    assert_eq!(
        answers(&reopens),
        [
            format!("✓ Reopened {closed}: closed"),
            format!("✓ Reopened {closed}: closed"),
        ]
    );
    assert_eq!(waiting(), 6);
    let deletes = a.run(["delete", &deleted, &deleted, "--offline", "--json"]);
    assert_eq!(deletes.returncode(), 0, "{}", deletes.stderr);
    let deletes = parse_json(&deletes.stdout).unwrap();
    for item in deletes.as_array().unwrap() {
        assert_eq!(item["status"].as_str(), Some("open"), "{item:?}");
        assert!(item.get("unchanged").is_none(), "{item:?}");
    }
    assert_eq!(waiting(), 8);
    for (key, status) in [
        (&claimed, "in_progress"),
        (&open, "open"),
        (&closed, "closed"),
        (&deleted, "open"),
    ] {
        assert_eq!(a.show(key)["status"].as_str(), Some(status), "{key}");
    }
}

#[test]
fn an_issue_named_again_is_as_its_event_left_it_though_events_off_the_frontier_wait() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let [claimed, open] = ["claimed", "open"].map(|t| create(&a, t, None));
    ok(&a, &["claim", &claimed, "--offline"]);
    // Two events, each naming the other as its cause: both wait, and neither is on the frontier,
    // so the events written after them apply.
    let [first, second] = [(); 2].map(|()| uuid7(&Clock::system()));
    let frontier = a.bead(["info"]).unwrap()["frontier"].clone();
    let event = |id: &str, cause: &str| {
        json!({"v": 1, "id": id, "replica": "feedface", "deps": [cause],
               "actor": {"agent": "newer"}, "op": "set", "issue": id,
               "data": {"fields": {"title": "x"}}})
    };
    let folder = a.store().join("events").join("feedface");
    std::fs::create_dir_all(&folder).unwrap();
    let lines: String = [event(&first, &second), event(&second, &first)]
        .iter()
        .map(|record| write(record, Format::ASCII).unwrap() + "\n")
        .collect();
    std::fs::write(folder.join("000001.jsonl"), lines).unwrap();
    let info = a.bead(["info"]).unwrap();
    assert_eq!(info["waiting_events"].as_i64(), Some(2));
    assert_eq!(info["frontier"], frontier);
    let closes = ok(&a, &["close", &claimed, &open, &claimed, "--offline"]);
    assert_eq!(
        answers(&closes),
        [
            format!("✓ Closed {claimed}: claimed"),
            format!("✓ Closed {open}: open"),
            format!("✓ Already closed {claimed}: claimed"),
        ]
    );
    assert_eq!(a.show(&claimed)["status"].as_str(), Some("closed"));
}

#[test]
fn an_issue_named_again_is_as_the_events_before_left_it_though_they_did_not_see_everything() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let alpha = create(&a, "alpha", None);
    let uid = a.bead(["info"]).unwrap()["frontier"][0]
        .as_str()
        .unwrap()
        .to_owned();
    create(&a, "beta", None);
    // Another machine set alpha's status without having seen beta's create, and two events naming
    // each other, one of them that change, wait: the change is on no head's past, so the close
    // built on the frontier does not see it, and alpha's status stays disputed after it.
    let [change, first, second] = [(); 3].map(|()| uuid7(&Clock::system()));
    let event = |id: &str, deps: &[&str], issue: &str, data: Json| {
        let deps = Json::Array(deps.iter().map(|dep| Json::from(*dep)).collect());
        json!({"v": 1, "id": id, "replica": "feedface", "deps": deps,
               "actor": {"agent": "other"}, "op": "set", "issue": issue, "data": data})
    };
    let title = || json!({"fields": {"title": "x"}});
    let folder = a.store().join("events").join("feedface");
    std::fs::create_dir_all(&folder).unwrap();
    let lines: String = [
        event(
            &change,
            &[&uid],
            &uid,
            json!({"fields": {"status": "in_progress"}}),
        ),
        event(&first, &[&second, &change], &first, title()),
        event(&second, &[&first], &second, title()),
    ]
    .iter()
    .map(|record| write(record, Format::ASCII).unwrap() + "\n")
    .collect();
    std::fs::write(folder.join("000001.jsonl"), lines).unwrap();
    let info = a.bead(["info"]).unwrap();
    assert_eq!(info["waiting_events"].as_i64(), Some(2));
    assert_eq!(info["frontier"].as_array().map(<[Json]>::len), Some(1));
    // this machine's records
    let own = || {
        let me = a.bead(["info"]).unwrap()["machine"]
            .as_str()
            .unwrap()
            .to_owned();
        (std::fs::read_dir(a.store().join("events").join(me)).unwrap())
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .map(|text| text.lines().count())
            .sum::<usize>()
    };
    let before = own();
    let closes = a.run(["close", &alpha, &alpha, "--offline", "--json"]);
    assert_eq!(closes.returncode(), 0, "{}", closes.stderr);
    let closes = parse_json(&closes.stdout).unwrap();
    let unchanged: Vec<bool> = (closes.as_array().unwrap().iter())
        .map(|item| item.get("unchanged").is_some())
        .collect();
    assert_eq!(unchanged, [false, false]);
    assert_eq!(own() - before, 2);
}
