//! A machine's journal chain beside events replay does not apply: each write builds on the
//! machine's last record, which an event pending in a cycle of causes keeps off the frontier.

use std::path::Path;

use beadroll_test_support::{Json, Machine, json, parse_json};

/// The id of `machine`'s last record in its journal's order: the event its next write builds on.
fn tail_of(machine: &Machine) -> String {
    let folder = machine.store().join("events").join(machine.id());
    let mut segments: Vec<_> = (std::fs::read_dir(&folder).expect("the machine's journal"))
        .map(|entry| entry.expect("a segment").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    segments.sort();
    let last = segments.last().expect("a segment");
    let read = std::fs::read_to_string(last).expect("the segment");
    let line = read.lines().last().expect("a record");
    let event = parse_json(line).expect("an event");
    event["id"].as_str().expect("an id").to_owned()
}

/// Writes, as another machine's journal in `machine`'s store, two events each waiting on the
/// other, the first building on `on` too: a cycle of causes replay never applies.
fn plant_cycle(machine: &Machine, on: &str) {
    let one = beadroll_test_support::events::id_at(1_790_000_000_001, 1);
    let two = beadroll_test_support::events::id_at(1_790_000_000_002, 2);
    let said = |id: &str, deps: Vec<&str>, text: &str| -> Json {
        let deps: Vec<Json> = deps.into_iter().map(Json::from).collect();
        json!({
            "v": 1, "id": id, "replica": "0000000f", "deps": deps,
            "actor": {"agent": "test", "thread": "f"},
            "op": "comment", "issue": on, "data": {"text": text}
        })
    };
    let lines = [
        said(&one, vec![on, &two], "one"),
        said(&two, vec![&one], "two"),
    ];
    let folder = machine.store().join("events").join("0000000f");
    private_dir(&folder);
    let text = beadroll_store::encode_events(&lines).expect("journal lines");
    std::fs::write(folder.join("000001.jsonl"), text).expect("the foreign journal");
}

/// Makes `folder` with only its owner able to read it, as the store keeps its own.
fn private_dir(folder: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(folder).expect("the folder");
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700)).expect("its mode");
}

/// That `machine`'s next writes apply: an issue, shown, and a child under it.
fn writes_apply(machine: &Machine) {
    let made = machine.bead(["create", "y"]).expect("an issue");
    let key = made["id"].as_str().expect("a key").to_owned();
    let shown = machine.show(&key);
    assert_eq!(shown["title"], Json::from("y"), "{shown:?}");
    let child = machine
        .bead(["create", "z", "--parent", &key])
        .expect("a child");
    assert_eq!(child["id"], Json::from(format!("{key}.1")), "{child:?}");
}

/// A machine's write builds on its last record while a foreign event in a cycle of causes builds
/// on it, keeping it off the frontier: replay applies the write, where one naming the frontier
/// alone broke its journal chain and was set aside, applied by nobody (AT102).
#[test]
fn a_write_beside_a_pending_cycle_builds_on_the_machines_last_record() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    plant_cycle(&a, &tail_of(&a));
    writes_apply(&a);
}

/// The record a write builds on is the last applicable in the journal's order, not the greatest
/// id: a record written with the clock turned back has a smaller id than the one before it, and a
/// write building on that one broke the chain at the last (AT102).
#[test]
fn a_write_builds_on_the_last_record_where_the_clock_went_back() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    let before = tail_of(&a);
    let back = a
        .cmd(["create", "w"])
        .env("BEAD_FAILPOINTS", "clock=offset(-1d)")
        .json();
    assert!(back.is_some(), "a create with the clock turned back");
    let tail = tail_of(&a);
    // the record written last has the smaller id: the greatest is the one before it
    assert!(tail < before, "{tail} sorts after {before}");
    plant_cycle(&a, &tail);
    writes_apply(&a);
}

/// Appends to `machine`'s own journal a comment on `issue` built on its last record and on an
/// event no journal holds, which waits for it: the id of the comment, its last record now.
fn append_waiting(machine: &Machine, issue: &str) -> String {
    let tail = tail_of(machine);
    let waiting = beadroll_test_support::events::id_at(1_790_000_000_010, 10);
    let missing = beadroll_test_support::events::id_at(1_790_000_000_011, 11);
    let event = json!({
        "v": 1, "id": (waiting.as_str()), "replica": (machine.id().as_str()),
        "deps": [(tail.as_str()), (missing.as_str())],
        "actor": {"agent": "test", "thread": "a"},
        "op": "comment", "issue": issue, "data": {"text": "waits"}
    });
    let folder = machine.store().join("events").join(machine.id());
    let mut segments: Vec<_> = (std::fs::read_dir(&folder).expect("the machine's journal"))
        .map(|entry| entry.expect("a segment").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    segments.sort();
    let last = segments.last().expect("a segment");
    let mut text = std::fs::read(last).expect("the segment");
    text.extend(beadroll_store::encode_events([&event]).expect("a journal line"));
    std::fs::write(last, text).expect("the segment, appended to");
    waiting
}

/// An ownership attempt is in no journal's chain, and builds on the frontier alone: a release
/// beside this machine's last event, which waits for a cause and which a foreign head released
/// without reaching it names, applies, where one naming that event too waited with it and left
/// the claim held (AT102).
#[test]
fn a_release_beside_a_waiting_last_event_builds_on_the_frontier_alone() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let made = a.bead(["create", "x"]).expect("an issue");
    let key = made["id"].as_str().expect("a key").to_owned();
    a.bead(["claim", &key]).expect("a claim");
    assert!(a.show(&key)["claim"]["owner"].as_str().is_some(), "claimed");
    // what the planted comments are about matters not: none of them applies
    let about = tail_of(&a);
    let waiting = append_waiting(&a, &about);
    // one id with two contents, each building on the waiting event
    let head = beadroll_test_support::events::id_at(1_790_000_000_012, 12);
    let said = |text: &str| -> Json {
        json!({
            "v": 1, "id": (head.as_str()), "replica": "0000000f", "deps": [(waiting.as_str())],
            "actor": {"agent": "test", "thread": "f"},
            "op": "comment", "issue": (about.as_str()), "data": {"text": text}
        })
    };
    let folder = a.store().join("events").join("0000000f");
    private_dir(&folder);
    let lines = [said("one content"), said("another")];
    let text = beadroll_store::encode_events(&lines).expect("journal lines");
    std::fs::write(folder.join("000001.jsonl"), text).expect("the foreign journal");
    a.bead(["release", &key]).expect("a release");
    let shown = a.show(&key);
    assert!(shown["claim"]["owner"].as_str().is_none(), "{shown:?}");
}
