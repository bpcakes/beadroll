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
/// alone broke its journal chain and was set aside, applied by nobody (AT100).
#[test]
fn a_write_beside_a_pending_cycle_builds_on_the_machines_last_record() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    plant_cycle(&a, &tail_of(&a));
    writes_apply(&a);
}

/// The last record a write builds on is the last in the journal's order, not the greatest id: a
/// record written with the clock turned back has a smaller id than the one before it, and a write
/// building on that one broke the chain at the last (AT100).
#[test]
fn a_write_builds_on_the_last_record_where_the_clock_went_back() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    a.bead(["create", "x"]);
    let back = a
        .cmd(["create", "w"])
        .env("BEAD_FAILPOINTS", "clock=offset(-1d)")
        .json();
    assert!(back.is_some(), "a create with the clock turned back");
    plant_cycle(&a, &tail_of(&a));
    writes_apply(&a);
}
