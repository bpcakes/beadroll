//! Identity and integrity: who made every event, and what bead does with a damaged line, a forged
//! event or a copied machine identity.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use beadroll_core::json::{Format, write};
use beadroll_test_support::{Json, Machine, code, json, parse_json};

use super::{column, id, length, segments, settle, two_machines};

/// The version of the UUID `text`, as Python's `uuid.UUID(text).version` reads it: `None` unless
/// it has the RFC 4122 variant. Text that is no UUID at all fails the test, as the constructor
/// raises.
fn uuid_version(text: &str) -> Option<u128> {
    let hex = text.replace("urn:", "").replace("uuid:", "");
    let hex = hex.trim_matches(['{', '}']).replace('-', "");
    assert_eq!(
        hex.len(),
        32,
        "badly formed hexadecimal UUID string: {text}"
    );
    let value = u128::from_str_radix(&hex, 16).unwrap_or_else(|error| panic!("{text}: {error}"));
    let rfc_4122 = (value >> 62) & 0b11 == 0b10;
    rfc_4122.then_some((value >> 76) & 0xf)
}

/// `shutil.copytree(from, to)`: `to`, which must not exist yet, becomes a copy of the directory
/// `from`, its files' modes and times kept.
fn copytree(from: &Path, to: &Path) {
    assert!(!to.exists(), "{} exists", to.display());
    let status = Command::new("cp")
        .arg("-pR")
        .arg(from)
        .arg(to)
        .status()
        .unwrap();
    assert!(status.success(), "cp -pR {} failed", from.display());
}

/// Every event carries the machine, agent and thread that made it, and its id is a UUIDv7.
#[test]
fn every_event_carries_machine_agent_and_thread() {
    let sandbox = crate::sandbox();
    let (a, b, x, _) = two_machines(&sandbox);
    b.cmd(["comments", "add", &x, "from a claude session"])
        .agent("claude")
        .thread("0f0f0f0f-claude-session")
        .json();
    settle(&[&a, &b]);
    let rows = a.bead(["log", &x]).unwrap();
    let actors: Vec<(Json, Json)> = column(&rows, "agent")
        .into_iter()
        .zip(column(&rows, "thread"))
        .collect();
    let expected = [
        ("codex", "a-thread-0001"),
        ("claude", "0f0f0f0f-claude-session"),
    ]
    .map(|(agent, thread)| (Json::from(agent), Json::from(thread)));
    assert_eq!(actors, expected);
    let machines = column(&rows, "machine");
    let machines: BTreeSet<&str> = machines.iter().map(|m| m.as_str().unwrap()).collect();
    assert_eq!(machines.len(), 2, "{machines:?}");
    assert_eq!(a.show(&x)["origin"]["agent"].as_str(), Some("codex"));
    assert_eq!(length(&a.bead(["machines"]).unwrap()), 2);
    let event = rows[0]["event"].as_str().unwrap();
    assert_eq!(uuid_version(event), Some(7), "{event}");
}

/// A line of a segment that is not JSON is reported as one integrity problem, and the tracker
/// reads and writes on.
#[test]
fn damaged_lines_and_payloads_are_reported_not_fatal() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = id(&a.bead(["create", "survivor"]).unwrap());
    let segment = segments(&a.store()).remove(0);
    let text = std::fs::read_to_string(&segment).unwrap();
    let mut rows: Vec<&str> = text.lines().collect();
    rows.insert(1, "this is not json");
    std::fs::write(&segment, rows.join("\n") + "\n").unwrap();
    assert_eq!(a.show(&key)["title"].as_str(), Some("survivor"));
    assert_eq!(length(&a.bead(["info"]).unwrap()["integrity"]), 1);
    a.bead(["create", "still writable"]);
    assert_eq!(a.bead(["list"]).unwrap()["total"].as_i64(), Some(2));
}

/// A forged event under the id of a real one, in a segment of a machine of its own, is reported
/// as one integrity problem, with both variants kept, and the issue it would change is set aside;
/// nothing else waits behind it, and the tracker carries on.
#[test]
fn one_corrupt_event_does_not_stop_the_tracker() {
    let sandbox = crate::sandbox();
    let (a, _, x, y) = two_machines(&sandbox);
    let store = a.store();
    let mut events = Vec::new();
    for path in segments(&store) {
        let text = std::fs::read_to_string(path).unwrap();
        events.extend(text.lines().map(|row| parse_json(row).unwrap()));
    }
    let victim = events
        .iter()
        .find(|e| e["op"].as_str() == Some("create") && title(e).as_str() == Some("Issue X"))
        .expect("the event that created Issue X");
    let mut data = victim["data"].as_object().unwrap().clone();
    data.insert("fields".into(), json!({"title": "forged"}));
    let mut forged = victim.as_object().unwrap().clone();
    forged.insert("replica".into(), Json::from("ffffffff"));
    forged.insert("data".into(), Json::from(data));
    let folder = store.join("events").join("ffffffff");
    std::fs::create_dir(&folder).unwrap();
    let line = write(&Json::from(forged), Format::ASCII).unwrap() + "\n";
    std::fs::write(folder.join("000001.jsonl"), line).unwrap();
    let info = a.bead(["info"]).unwrap();
    // the ambiguous event is reported, with both variants kept
    assert_eq!(length(&info["integrity"]), 1, "{info:?}");
    // and nothing else is held up behind it
    assert_eq!(info["waiting_events"].as_i64(), Some(0));
    assert_eq!(a.show(&y)["title"].as_str(), Some("Issue Y"));
    assert_eq!(code(&a.run(["show", &x, "--json"])), "ISSUE_NOT_FOUND");
    a.bead(["create", "work continues"]);
    assert_eq!(a.ready().len(), 2);
}

/// The title a `create` event gives its issue, `event["data"]["fields"]["title"]`; an event
/// without one fails the test, as the Python test's lookup raises.
fn title(event: &Json) -> &Json {
    let fields = event.get("data").and_then(|data| data.get("fields"));
    let title = fields.and_then(|fields| fields.get("title"));
    title.unwrap_or_else(|| panic!("no title in {event:?}"))
}

/// A copy of a machine's home, a restored backup or a cloned disk, has the same identity in two
/// places: its sync is refused as a fork of the journal, and everyone else carries on.
#[test]
fn a_copied_machine_identity_is_detected() {
    let sandbox = crate::sandbox();
    let (a, b, _, _) = two_machines(&sandbox);
    let twin = Machine::new(&sandbox, "twin");
    // a restored backup or cloned disk: same identity, two places
    copytree(a.home(), twin.home());
    a.bead(["create", "made on the original"]);
    twin.bead(["create", "made on the copy"]);
    a.bead(["sync"]);
    assert_eq!(code(&twin.run(["sync", "--json"])), "JOURNAL_FORK");
    b.bead(["sync"]);
    // everyone else carries on
    assert_eq!(b.bead(["list"]).unwrap()["total"].as_i64(), Some(3));
}
